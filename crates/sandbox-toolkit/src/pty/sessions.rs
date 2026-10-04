use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::pty::session::Attachment;
use crate::pty::session::Session;

/// How often sessions are checked for expiry; a second is far under both
/// deadlines.
const TICK: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub struct Sessions {
    sessions: Arc<Mutex<HashMap<String, Arc<Session>>>>,
}

impl Sessions {
    pub fn new() -> Self {
        let sessions = Arc::new(Mutex::new(HashMap::new()));
        tokio::spawn(reap(Arc::clone(&sessions)));

        Self { sessions }
    }

    pub async fn create(&self, session: Session) -> String {
        let id = new_id();
        self.sessions
            .lock()
            .await
            .insert(id.clone(), Arc::new(session));

        id
    }

    pub async fn attach(&self, scope: &str, id: &str) -> Option<(Arc<Session>, Attachment)> {
        let session = self.sessions.lock().await.get(id).cloned()?;
        let attachment = session.claim(scope)?;

        Some((session, attachment))
    }
}

impl Default for Sessions {
    fn default() -> Self {
        Self::new()
    }
}

async fn reap(sessions: Arc<Mutex<HashMap<String, Arc<Session>>>>) {
    let mut ticker = tokio::time::interval(TICK);

    loop {
        ticker.tick().await;

        let expired = {
            let mut sessions = sessions.lock().await;
            sessions.retain(|_, session| !session.reclaimed());

            sessions
                .values()
                .filter(|session| session.expired())
                .cloned()
                .collect::<Vec<_>>()
        };

        for session in expired {
            session.reclaim();
        }
    }
}

/// Session ids name an endpoint, so they are unique within one server run: the
/// process id separates runs, the counter separates sessions.
fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);

    format!("pty-{}-{serial}", std::process::id())
}
