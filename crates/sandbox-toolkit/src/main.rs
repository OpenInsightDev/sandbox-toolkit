use clap::{Parser, Subcommand};
use sandbox_toolkit_utils::shutdown;
use tracing_subscriber::EnvFilter;

mod binary;
mod exec;
mod fs;
mod http;
mod mcp;
mod path;
mod plugin;
mod pty;
mod skill;
mod tus;
mod workspace;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Serve {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 3000)]
        port: u16,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    match Cli::parse().command {
        Command::Serve { host, port } => {
            let bin = binary::materialize().await?;
            // Reached only once tusd takes requests, so the mount answers from
            // the first request on.
            let tus = tus::Sidecar::start(&bin).await?;

            let registry = workspace::Registry::new().await?;
            let state = http::AppState::new(registry, bin, tus.upstream(), tus.uploads());
            // Serving comes to rest before the sidecar does, so a request that
            // is still running is answered by a tusd that is still up.
            http::serve((host, port), state, shutdown::requested()).await;
            tus.stop().await?;
        }
    }

    Ok(())
}
