use anyhow::Result;
use clap::Parser;
use oss_context::config::{AppConfig, CliOverrides};
use oss_context::discovery;
use oss_context::tools::OssContextServer;

#[derive(Parser, Debug)]
#[command(name = "oss-context", about = "Java library documentation MCP server")]
struct Cli {
    /// Transport mode: stdio or sse
    #[arg(long, default_value = "stdio")]
    transport: String,

    /// SSE port
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

    /// Disable auto-discovery
    #[arg(long)]
    no_auto_discover: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "oss_context=info".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();

    let cli_overrides = CliOverrides {
        transport: cli.transport,
        port: cli.port,
        local_repos: cli.local_repos,
        remote_repos: cli.remote_repos,
        cache_dir: cli.cache_dir,
        no_auto_discover: cli.no_auto_discover,
    };

    let file_config = AppConfig::load_file_config();
    let discovered = if cli_overrides.no_auto_discover {
        Default::default()
    } else {
        let cwd = std::env::current_dir().unwrap_or_default();
        discovery::discover(&cwd)
    };

    let config = AppConfig::merge(&cli_overrides, &file_config, &discovered);
    tracing::info!("Configuration loaded, cache_dir: {:?}", config.cache_dir);

    let server = OssContextServer::new(config.clone());

    match config.transport {
        oss_context::config::Transport::Stdio => {
            oss_context::transport::serve_stdio(server).await?;
        }
        oss_context::config::Transport::Sse => {
            oss_context::transport::serve_sse(server, config.port).await?;
        }
    }

    Ok(())
}
