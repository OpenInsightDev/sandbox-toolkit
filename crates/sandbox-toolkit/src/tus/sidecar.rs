use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::net::UnixStream;
use tokio::process::{Child, Command};

use crate::binary::path;

use super::Upstream;
use super::uploads::Uploads;

/// The materialized executable this sidecar runs.
const BIN: &str = "tusd";

const SOCKET: &str = "tus.sock";
const UPLOADS: &str = "tus";

/// tusd serves the mount the proxy forwards under this base path.
const BASE_PATH: &str = "/tus";

/// How long tusd is given to bind its socket before the start is called off.
const READY: Duration = Duration::from_secs(10);

/// How long tusd is given to exit once it is asked to stop. tusd bounds its own
/// shutdown with the same timeout, so reaching this means it is wedged.
const SHUTDOWN: Duration = Duration::from_secs(10);

/// How often the socket is dialled while waiting for tusd to take requests.
const POLL: Duration = Duration::from_millis(20);

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("tusd did not listen within {READY:?}")]
    Unready,
    #[error("tusd exited with {0}")]
    Exited(ExitStatus),
}

/// The tusd the service runs beside itself.
pub struct Sidecar {
    child: Child,
    dir: PathBuf,
}

impl Sidecar {
    /// Starts tusd on the socket and upload directory it is pinned to, and
    /// returns once it takes requests.
    pub async fn start(bin: &Path) -> Result<Self, Error> {
        let dir = path::cache()?;
        tokio::fs::create_dir_all(&dir).await?;

        let socket = dir.join(SOCKET);
        let mut child = Command::new(bin.join(BIN))
            .arg("-unix-sock")
            .arg(&socket)
            .arg("-base-path")
            .arg(BASE_PATH)
            .arg("-behind-proxy")
            .arg("-upload-dir")
            .arg(dir.join(UPLOADS))
            // tusd stops only by being signalled, so a path that returns
            // before `stop` would leave the process behind.
            .kill_on_drop(true)
            .spawn()?;

        Self::wait_listening(&socket, &mut child).await?;

        Ok(Self { child, dir })
    }

    /// tusd creates the socket it serves on before it accepts anything, so a
    /// connection that goes through is what says it is up.
    async fn wait_listening(socket: &Path, child: &mut Child) -> Result<(), Error> {
        let deadline = Instant::now() + READY;

        loop {
            if UnixStream::connect(socket).await.is_ok() {
                return Ok(());
            }
            if let Some(status) = child.try_wait()? {
                return Err(Error::Exited(status));
            }
            if Instant::now() >= deadline {
                return Err(Error::Unready);
            }

            tokio::time::sleep(POLL).await;
        }
    }

    /// The endpoint the `/tus` mount proxies to.
    pub fn upstream(&self) -> Upstream {
        Upstream::new(self.dir.join(SOCKET))
    }

    /// The uploads staged in the sidecar's upload directory, which
    /// `POST ?type=commit` moves onto a path.
    pub fn uploads(&self) -> Uploads {
        Uploads::new(self.dir.join(UPLOADS))
    }

    /// Ends tusd, waiting at most `SHUTDOWN` for it to end by itself.
    pub async fn stop(mut self) -> Result<(), Error> {
        if let Some(pid) = self.child.id() {
            // A child is asked to stop by hand: tokio only kills, and the std
            // spelling of this (`ChildExt::send_signal`) is still nightly.
            // SAFETY: the pid names the child this handle owns.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        }

        match tokio::time::timeout(SHUTDOWN, self.child.wait()).await {
            Ok(status) => {
                status?;
            }
            Err(_) => self.child.kill().await?,
        }

        Ok(())
    }
}
