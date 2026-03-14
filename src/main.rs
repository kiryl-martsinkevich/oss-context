use anyhow::Result;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "oss-context", about = "Java library documentation MCP server")]
struct Cli {
    /// Transport mode
    #[arg(long, default_value = "stdio")]
    transport: String,

    /// SSE port (only used with --transport sse)
    #[arg(long, default_value = "8080")]
    port: u16,

    /// Additional local repo path (can be repeated)
    #[arg(long = "local-repo")]
    local_repos: Vec<String>,

    /// Additional remote repo URL (can be repeated)
    #[arg(long = "remote-repo")]
    remote_repos: Vec<String>,

    /// Cache directory for SQLite databases
    #[arg(long)]
    cache_dir: Option<String>,

    /// Disable auto-discovery of repos
    #[arg(long)]
    no_auto_discover: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    tracing::info!(?cli, "Starting oss-context");

    Ok(())
}
