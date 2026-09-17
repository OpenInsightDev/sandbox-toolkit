mod jsonrpc;
mod tools;

use std::process::ExitCode;

use clap::Parser;

/// The sandbox toolkit server.
#[derive(Debug, Parser)]
#[command(name = "sandbox-toolkit", version, about)]
struct Cli {
    /// Materialize the bundled tools into a stable temporary directory at
    /// startup, instead of only reporting where they were compiled.
    #[arg(long)]
    tools: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    if cli.tools {
        match tools::materialize().await {
            Ok(dir) => {
                println!("tools written to {}", dir.display());
                for tool in tools::bundled() {
                    println!("  {:<6} {}", tool.name, dir.join(tool.name).display());
                }
            }
            Err(error) => {
                eprintln!("failed to materialize bundled tools: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        println!("bundled tools:");
        for tool in tools::bundled() {
            println!("  {:<6} {}", tool.name, tool.path);
        }
    }

    ExitCode::SUCCESS
}
