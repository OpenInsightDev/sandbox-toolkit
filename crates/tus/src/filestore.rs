use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncRead, AsyncWriteExt};

use crate::datastore::{
    BoxFuture, Concater, DataStore, FileInfo, StoreComposer, Terminater, Upload,
};
use crate::error::{Error, Result};

/// Storage on the local file system: `{id}` holds the raw bytes and `{id}.info`
/// the JSON descriptor. The offset is always derived from the binary size, so
/// no extra bookkeeping is needed after a write.
#[derive(Debug, Clone)]
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Registers this store as the core store, terminater and concater.
    pub fn use_in(self: &Arc<Self>, composer: StoreComposer) -> StoreComposer {
        composer
            .with_core(self.clone())
            .with_terminater(self.clone())
            .with_concater(self.clone())
    }

    fn bin_path(&self, id: &str) -> PathBuf {
        self.path.join(id)
    }

    fn info_path(&self, id: &str) -> PathBuf {
        self.path.join(format!("{id}.info"))
    }
}

impl DataStore for FileStore {
    fn create_upload(&self, mut info: FileInfo) -> BoxFuture<'_, Result<Box<dyn Upload>>> {
        Box::pin(async move {
            if info.id.is_empty() {
                info.id = generate_id();
            }

            let bin_path = self.bin_path(&info.id);
            let info_path = self.info_path(&info.id);

            create_file(&bin_path, None).await?;

            let upload = FileUpload {
                info: StdMutex::new(info),
                bin_path,
                info_path,
            };
            upload.write_info().await?;

            Ok(Box::new(upload) as Box<dyn Upload>)
        })
    }

    fn get_upload<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Box<dyn Upload>>> {
        Box::pin(async move {
            let info_path = self.info_path(id);
            let data = match fs::read(&info_path).await {
                Ok(data) => data,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    return Err(Error::not_found());
                }
                Err(err) => return Err(Error::internal(err.to_string())),
            };

            let mut info: FileInfo =
                serde_json::from_slice(&data).map_err(|err| Error::internal(err.to_string()))?;

            let bin_path = self.bin_path(&info.id);
            let metadata = match fs::metadata(&bin_path).await {
                Ok(metadata) => metadata,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    return Err(Error::not_found());
                }
                Err(err) => return Err(Error::internal(err.to_string())),
            };

            info.offset = metadata.len();

            Ok(Box::new(FileUpload {
                info: StdMutex::new(info),
                bin_path,
                info_path,
            }) as Box<dyn Upload>)
        })
    }
}

impl Terminater for FileStore {
    fn terminate<'a>(&'a self, upload: &'a dyn Upload) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let info = upload.info().await?;
            remove_if_exists(&self.bin_path(&info.id)).await?;
            remove_if_exists(&self.info_path(&info.id)).await?;
            Ok(())
        })
    }
}

impl Concater for FileStore {
    fn concat_uploads<'a>(
        &'a self,
        upload: &'a dyn Upload,
        partial_uploads: &'a [Box<dyn Upload>],
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let final_info = upload.info().await?;
            let mut file = OpenOptions::new()
                .append(true)
                .open(self.bin_path(&final_info.id))
                .await
                .map_err(|err| Error::internal(err.to_string()))?;

            for partial in partial_uploads {
                let info = partial.info().await?;
                let mut src = fs::File::open(self.bin_path(&info.id))
                    .await
                    .map_err(|err| Error::internal(err.to_string()))?;
                tokio::io::copy(&mut src, &mut file)
                    .await
                    .map_err(|err| Error::internal(err.to_string()))?;
            }

            file.flush()
                .await
                .map_err(|err| Error::internal(err.to_string()))?;
            Ok(())
        })
    }
}

#[derive(Debug)]
struct FileUpload {
    info: StdMutex<FileInfo>,
    bin_path: PathBuf,
    info_path: PathBuf,
}

impl FileUpload {
    async fn write_info(&self) -> Result<()> {
        let data = {
            let info = self.info.lock().expect("file info poisoned");
            serde_json::to_vec(&*info).map_err(|err| Error::internal(err.to_string()))?
        };
        create_file(&self.info_path, Some(&data)).await
    }
}

impl Upload for FileUpload {
    fn info(&self) -> BoxFuture<'_, Result<FileInfo>> {
        Box::pin(async move { Ok(self.info.lock().expect("file info poisoned").clone()) })
    }

    fn write_chunk<'a>(
        &'a self,
        _offset: u64,
        src: &'a mut (dyn AsyncRead + Unpin + Send),
    ) -> BoxFuture<'a, Result<u64>> {
        Box::pin(async move {
            let mut file = OpenOptions::new()
                .append(true)
                .open(&self.bin_path)
                .await
                .map_err(|err| Error::internal(err.to_string()))?;

            let written = tokio::io::copy(src, &mut file)
                .await
                .map_err(|err| Error::internal(err.to_string()))?;

            file.flush()
                .await
                .map_err(|err| Error::internal(err.to_string()))?;

            self.info.lock().expect("file info poisoned").offset += written;

            Ok(written)
        })
    }

    fn finish(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

/// Creates or truncates `path`, creating missing parent directories. This
/// mirrors tusd's `createFile`, which also maps IDs containing slashes onto
/// subdirectories.
async fn create_file(path: &Path, content: Option<&[u8]>) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|err| Error::internal(err.to_string()))?;
    }

    let mut file = fs::File::create(path)
        .await
        .map_err(|err| Error::internal(err.to_string()))?;

    if let Some(content) = content {
        file.write_all(content)
            .await
            .map_err(|err| Error::internal(err.to_string()))?;
    }

    file.flush()
        .await
        .map_err(|err| Error::internal(err.to_string()))?;
    Ok(())
}

async fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(Error::internal(err.to_string())),
    }
}

/// 128 bits of randomness, hex-encoded. Falls back to time plus a process-local
/// counter if `/dev/urandom` cannot be read.
fn generate_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let mut bytes = [0u8; 16];
    let random = std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .is_ok();

    if !random {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or_default();
        bytes[..8].copy_from_slice(&nanos.to_le_bytes());
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
        bytes[8..].copy_from_slice(&counter.to_le_bytes());
    }

    let mut id = String::with_capacity(32);
    for byte in bytes {
        let _ = write!(id, "{byte:02x}");
    }
    id
}
