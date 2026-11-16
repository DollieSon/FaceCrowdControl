use clap::Parser;
use fcc_server::config::{Cli, Command};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Settings can come from a .env file next to where the server is started.
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("fcc_server=info,tower_http=warn")),
        )
        .init();

    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => fcc_server::serve(cli.settings.normalized()).await,
        Command::CopyDb { from, to } => fcc_server::copy_db(&from, &to).await,
    }
}
