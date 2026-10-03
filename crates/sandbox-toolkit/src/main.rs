use clap::{Parser, Subcommand};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

mod exec;
mod http;
mod mcp;
mod path;
mod skill;
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
            let listener = TcpListener::bind((host.as_str(), port)).await?;
            tracing::info!(address = %listener.local_addr()?, "listening");

            let registry = workspace::Registry::new().await?;
            http::serve(listener, http::AppState::new(registry)).await?;
        }
    }

    Ok(())
}
