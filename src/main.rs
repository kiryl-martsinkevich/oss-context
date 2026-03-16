use anyhow::Result;
use oss_context::config::{AppConfig, CliOverrides};
use oss_context::discovery;
use oss_context::tools::OssContextServer;

struct Cli {
    transport: String,
    port: u16,
    local_repos: Vec<String>,
    remote_repos: Vec<String>,
    cache_dir: Option<String>,
    no_auto_discover: bool,
}

impl Cli {
    fn parse() -> Cli {
        let args: Vec<String> = std::env::args().collect();
        let mut cli = Cli {
            transport: "stdio".to_string(),
            port: 8080,
            local_repos: Vec::new(),
            remote_repos: Vec::new(),
            cache_dir: None,
            no_auto_discover: false,
        };

        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--transport" => {
                    i += 1;
                    cli.transport = args.get(i).expect("--transport requires a value").clone();
                }
                "--port" => {
                    i += 1;
                    cli.port = args.get(i).expect("--port requires a value")
                        .parse().expect("--port must be a valid u16");
                }
                "--local-repo" => {
                    i += 1;
                    cli.local_repos.push(args.get(i).expect("--local-repo requires a value").clone());
                }
                "--remote-repo" => {
                    i += 1;
                    cli.remote_repos.push(args.get(i).expect("--remote-repo requires a value").clone());
                }
                "--cache-dir" => {
                    i += 1;
                    cli.cache_dir = Some(args.get(i).expect("--cache-dir requires a value").clone());
                }
                "--no-auto-discover" => {
                    cli.no_auto_discover = true;
                }
                "--help" | "-h" => {
                    eprintln!("oss-context - Java library documentation MCP server\n");
                    eprintln!("Usage: oss-context [OPTIONS]\n");
                    eprintln!("Options:");
                    eprintln!("  --transport <MODE>     Transport mode: stdio or sse [default: stdio]");
                    eprintln!("  --port <PORT>          SSE port [default: 8080]");
                    eprintln!("  --local-repo <PATH>    Additional local repo path (can be repeated)");
                    eprintln!("  --remote-repo <URL>    Additional remote repo URL (can be repeated)");
                    eprintln!("  --cache-dir <DIR>      Cache directory for SQLite databases");
                    eprintln!("  --no-auto-discover     Disable auto-discovery");
                    eprintln!("  -h, --help             Print help");
                    std::process::exit(0);
                }
                other => {
                    eprintln!("Unknown argument: {other}");
                    std::process::exit(1);
                }
            }
            i += 1;
        }

        cli
    }
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
