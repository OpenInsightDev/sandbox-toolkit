use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncRead;

use crate::error::Result;

pub type Metadata = BTreeMap<String, String>;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileInfo {
    pub id: String,
    pub size: u64,
    pub offset: u64,
    pub metadata: Metadata,
    pub is_partial: bool,
    pub is_final: bool,
    pub partial_uploads: Vec<String>,
}

/// A single upload resource. The handler validates the offset and clamps the
/// body before calling [`Upload::write_chunk`].
pub trait Upload: Send + Sync + 'static {
    fn info(&self) -> BoxFuture<'_, Result<FileInfo>>;

    fn write_chunk<'a>(
        &'a self,
        offset: u64,
        src: &'a mut (dyn AsyncRead + Unpin + Send),
    ) -> BoxFuture<'a, Result<u64>>;

    fn finish(&self) -> BoxFuture<'_, Result<()>>;
}

/// The core store. `GetUpload` must fail with an `UploadNotFound` [`Error`] for
/// an unknown id.
pub trait DataStore: Send + Sync + 'static {
    fn create_upload(&self, info: FileInfo) -> BoxFuture<'_, Result<Box<dyn Upload>>>;

    fn get_upload<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Box<dyn Upload>>>;
}

/// Confers the `termination` extension.
pub trait Terminater: Send + Sync + 'static {
    fn terminate<'a>(&'a self, upload: &'a dyn Upload) -> BoxFuture<'a, Result<()>>;
}

/// Confers the `concatenation` extension.
pub trait Concater: Send + Sync + 'static {
    fn concat_uploads<'a>(
        &'a self,
        upload: &'a dyn Upload,
        partial_uploads: &'a [Box<dyn Upload>],
    ) -> BoxFuture<'a, Result<()>>;
}

/// Confers serialized access to an upload across processes.
pub trait Locker: Send + Sync + 'static {
    fn new_lock(&self, id: &str) -> Result<Box<dyn Lock>>;
}

pub trait Lock: Send + Sync + 'static {
    /// Invokes `request_unlock` when another caller wants the lock, so the
    /// holder can release it early. The callback outlives the caller so that a
    /// poller can hold it after `lock` returns.
    fn lock(
        &self,
        request_unlock: Box<dyn Fn() + Send + Sync + 'static>,
    ) -> BoxFuture<'_, Result<()>>;

    fn unlock(&self) -> BoxFuture<'_, Result<()>>;
}

/// The core store plus the optional extensions, mirroring tusd's `StoreComposer`.
#[derive(Clone, Default)]
pub struct StoreComposer {
    core: Option<Arc<dyn DataStore>>,
    terminater: Option<Arc<dyn Terminater>>,
    concater: Option<Arc<dyn Concater>>,
    locker: Option<Arc<dyn Locker>>,
}

impl StoreComposer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_core(mut self, core: Arc<dyn DataStore>) -> Self {
        self.core = Some(core);
        self
    }

    pub fn with_terminater(mut self, terminater: Arc<dyn Terminater>) -> Self {
        self.terminater = Some(terminater);
        self
    }

    pub fn with_concater(mut self, concater: Arc<dyn Concater>) -> Self {
        self.concater = Some(concater);
        self
    }

    pub fn with_locker(mut self, locker: Arc<dyn Locker>) -> Self {
        self.locker = Some(locker);
        self
    }

    pub fn core(&self) -> Option<&Arc<dyn DataStore>> {
        self.core.as_ref()
    }

    pub fn terminater(&self) -> Option<&Arc<dyn Terminater>> {
        self.terminater.as_ref()
    }

    pub fn concater(&self) -> Option<&Arc<dyn Concater>> {
        self.concater.as_ref()
    }

    pub fn locker(&self) -> Option<&Arc<dyn Locker>> {
        self.locker.as_ref()
    }
}
