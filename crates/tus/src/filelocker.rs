use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::fs::{self, OpenOptions};
use tokio::task::JoinHandle;

use crate::datastore::{BoxFuture, Lock, Locker, StoreComposer};
use crate::error::{Error, Result};

const DEFAULT_HOLDER_POLL_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_ACQUIRER_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// File-system locks, mirroring tusd's `filelocker`: an exclusive `flock(2)` on
/// `{id}.lock` plus a `{id}.stop` file used to ask the holder to release.
#[derive(Debug, Clone)]
pub struct FileLocker {
    path: PathBuf,
    holder_poll_interval: Duration,
    acquirer_poll_interval: Duration,
}

impl FileLocker {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            holder_poll_interval: DEFAULT_HOLDER_POLL_INTERVAL,
            acquirer_poll_interval: DEFAULT_ACQUIRER_POLL_INTERVAL,
        }
    }

    pub fn use_in(self: &Arc<Self>, composer: StoreComposer) -> StoreComposer {
        composer.with_locker(self.clone())
    }

    fn lock_path(&self, id: &str) -> PathBuf {
        self.path.join(format!("{id}.lock"))
    }

    fn stop_path(&self, id: &str) -> PathBuf {
        self.path.join(format!("{id}.stop"))
    }
}

impl Locker for FileLocker {
    fn new_lock(&self, id: &str) -> Result<Box<dyn Lock>> {
        Ok(Box::new(FileUploadLock {
            locker: self.clone(),
            id: id.to_owned(),
            file: StdMutex::new(None),
            poller: StdMutex::new(None),
        }))
    }
}

struct FileUploadLock {
    locker: FileLocker,
    id: String,
    file: StdMutex<Option<fs::File>>,
    poller: StdMutex<Option<JoinHandle<()>>>,
}

impl Lock for FileUploadLock {
    fn lock(
        &self,
        request_unlock: Box<dyn Fn() + Send + Sync + 'static>,
    ) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let lock_path = self.locker.lock_path(&self.id);
            let stop_path = self.locker.stop_path(&self.id);

            let file = loop {
                let file = match OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(&lock_path)
                    .await
                {
                    Ok(file) => file,
                    // A missing parent directory means the upload does not exist.
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                        return Err(Error::not_found());
                    }
                    Err(err) => return Err(Error::internal(err.to_string())),
                };

                match try_lock_exclusive(&file) {
                    Ok(true) => break file,
                    Ok(false) => {
                        // Tell the current holder to stop, then retry.
                        create_stop_file(&stop_path).await?;
                        tokio::time::sleep(self.locker.acquirer_poll_interval).await;
                    }
                    Err(err) => return Err(err),
                }
            };

            let interval = self.locker.holder_poll_interval;
            let handle = tokio::spawn(async move {
                loop {
                    tokio::time::sleep(interval).await;
                    if fs::try_exists(&stop_path).await.unwrap_or(false) {
                        request_unlock();
                        return;
                    }
                }
            });

            *self.file.lock().expect("file lock poisoned") = Some(file);
            *self.poller.lock().expect("file lock poisoned") = Some(handle);

            Ok(())
        })
    }

    fn unlock(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if let Some(handle) = self.poller.lock().expect("file lock poisoned").take() {
                handle.abort();
            }

            if let Some(file) = self.file.lock().expect("file lock poisoned").take() {
                unlock_exclusive(&file)?;
            }

            // The error is ignored on purpose: the stop file may never exist.
            let _ = fs::remove_file(self.locker.stop_path(&self.id)).await;

            Ok(())
        })
    }
}

fn try_lock_exclusive(file: &fs::File) -> Result<bool> {
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }

    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
        return Ok(false);
    }

    Err(Error::internal(err.to_string()))
}

fn unlock_exclusive(file: &fs::File) -> Result<()> {
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    if result == 0 {
        return Ok(());
    }

    Err(Error::internal(std::io::Error::last_os_error().to_string()))
}

async fn create_stop_file(path: &Path) -> Result<()> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .await
        .map_err(|err| Error::internal(err.to_string()))?;
    Ok(())
}
