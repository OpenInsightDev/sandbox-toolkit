use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::Notify;

use crate::datastore::{BoxFuture, Lock, Locker, StoreComposer};
use crate::error::Result;

/// In-memory locks, mirroring tusd's `memorylocker`. A waiter signals the
/// holder through its `request_release` callback and then blocks until the
/// holder releases.
#[derive(Clone, Default)]
pub struct MemoryLocker {
    locks: Arc<StdMutex<HashMap<String, LockEntry>>>,
}

struct LockEntry {
    released: Arc<Notify>,
    request_release: Box<dyn Fn() + Send + Sync + 'static>,
}

impl MemoryLocker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn use_in(self: &Arc<Self>, composer: StoreComposer) -> StoreComposer {
        composer.with_locker(self.clone())
    }
}

impl Locker for MemoryLocker {
    fn new_lock(&self, id: &str) -> Result<Box<dyn Lock>> {
        Ok(Box::new(MemoryLock {
            locker: self.clone(),
            id: id.to_owned(),
        }))
    }
}

struct MemoryLock {
    locker: MemoryLocker,
    id: String,
}

impl Lock for MemoryLock {
    fn lock(
        &self,
        request_unlock: Box<dyn Fn() + Send + Sync + 'static>,
    ) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let mut request_unlock = Some(request_unlock);

            loop {
                let released = {
                    let mut locks = self.locker.locks.lock().expect("memory locker poisoned");
                    match locks.get(&self.id) {
                        None => {
                            locks.insert(
                                self.id.clone(),
                                LockEntry {
                                    released: Arc::new(Notify::new()),
                                    request_release: request_unlock
                                        .take()
                                        .expect("lock requested twice"),
                                },
                            );
                            return Ok(());
                        }
                        Some(entry) => Arc::clone(&entry.released),
                    }
                };

                // Register the waiter before signalling the holder, otherwise a
                // concurrent release could be missed.
                let notified = released.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();

                {
                    let locks = self.locker.locks.lock().expect("memory locker poisoned");
                    match locks.get(&self.id) {
                        Some(entry) => (entry.request_release)(),
                        // Released in the meantime; re-check from the top.
                        None => continue,
                    }
                }

                notified.await;
            }
        })
    }

    fn unlock(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let entry = self
                .locker
                .locks
                .lock()
                .expect("memory locker poisoned")
                .remove(&self.id);

            if let Some(entry) = entry {
                entry.released.notify_waiters();
            }

            Ok(())
        })
    }
}
