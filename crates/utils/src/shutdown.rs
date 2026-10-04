use tokio::signal::unix::{SignalKind, signal};

/// Resolves when the process is asked to stop, by `SIGTERM` or `SIGINT`.
///
/// A handler that cannot be installed is fatal: a process that ignored the
/// signal could only be stopped in a way that leaves what it spawned behind.
pub async fn requested() {
    let interrupt = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install a SIGINT handler");
    };
    let terminate = async {
        signal(SignalKind::terminate())
            .expect("failed to install a SIGTERM handler")
            .recv()
            .await;
    };

    tokio::select! {
        _ = interrupt => {}
        _ = terminate => {}
    }
}
