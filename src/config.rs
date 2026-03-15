use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct AppConfig {
    pub transport: Transport,
    pub port: u16,
    pub local_repo_paths: Vec<PathBuf>,
    pub remote_repos: Vec<RemoteRepo>,
    pub cache_dir: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum Transport {
    #[default]
    Stdio,
    Sse,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemoteRepo {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct FileConfig {
    pub local: Option<LocalFileConfig>,
    pub remote: Option<RemoteFileConfig>,
    pub storage: Option<StorageFileConfig>,
    pub query: Option<QueryFileConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LocalFileConfig {
    pub extra_paths: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RemoteFileConfig {
    pub extra_repos: Option<Vec<RemoteRepoEntry>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemoteRepoEntry {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StorageFileConfig {
    pub cache_dir: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct QueryFileConfig {
    pub default_limit: Option<usize>,
}

/// CLI inputs to merge into config
#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    pub transport: String,
    pub port: u16,
    pub local_repos: Vec<String>,
    pub remote_repos: Vec<String>,
    pub cache_dir: Option<String>,
    pub no_auto_discover: bool,
}

/// Discovered repo paths and remote URLs
#[derive(Debug, Clone, Default)]
pub struct DiscoveredConfig {
    pub local_repo_paths: Vec<PathBuf>,
    pub remote_repos: Vec<RemoteRepo>,
}

fn default_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("oss-context")
        .join("docs")
}

impl AppConfig {
    /// Merge: CLI > file config > discovered. Lists are merged, scalars overridden.
    pub fn merge(
        cli: &CliOverrides,
        file_config: &FileConfig,
        discovered: &DiscoveredConfig,
    ) -> Self {
        let transport = match cli.transport.as_str() {
            "sse" => Transport::Sse,
            _ => Transport::Stdio,
        };

        // Cache dir: CLI > file > default
        let cache_dir = cli
            .cache_dir
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| {
                file_config
                    .storage
                    .as_ref()
                    .and_then(|s| s.cache_dir.as_ref())
                    .map(|s| {
                        let expanded = shellexpand::tilde(s);
                        PathBuf::from(expanded.as_ref())
                    })
            })
            .unwrap_or_else(default_cache_dir);

        // Local repos: merge all sources
        let mut local_repo_paths: Vec<PathBuf> = if cli.no_auto_discover {
            vec![]
        } else {
            discovered.local_repo_paths.clone()
        };
        if let Some(ref local) = file_config.local {
            if let Some(ref extra) = local.extra_paths {
                local_repo_paths.extend(extra.iter().map(PathBuf::from));
            }
        }
        local_repo_paths.extend(cli.local_repos.iter().map(PathBuf::from));

        // Remote repos: merge all sources
        let mut remote_repos: Vec<RemoteRepo> = if cli.no_auto_discover {
            vec![]
        } else {
            discovered.remote_repos.clone()
        };
        if let Some(ref remote) = file_config.remote {
            if let Some(ref extra) = remote.extra_repos {
                remote_repos.extend(extra.iter().map(|e| RemoteRepo {
                    name: e.name.clone(),
                    url: e.url.clone(),
                }));
            }
        }
        remote_repos.extend(cli.remote_repos.iter().map(|url| RemoteRepo {
            name: "cli".to_string(),
            url: url.clone(),
        }));

        AppConfig {
            transport,
            port: cli.port,
            local_repo_paths,
            remote_repos,
            cache_dir,
        }
    }

    /// Load file config from default path
    pub fn load_file_config() -> FileConfig {
        let path = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("oss-context")
            .join("config.toml");

        if path.exists() {
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("Failed to read config file {}: {}", path.display(), e);
                    return FileConfig::default();
                }
            };
            match toml::from_str(&content) {
                Ok(config) => config,
                Err(e) => {
                    tracing::warn!("Failed to parse config file {}: {}", path.display(), e);
                    FileConfig::default()
                }
            }
        } else {
            FileConfig::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_defaults() {
        let cli = CliOverrides {
            transport: "stdio".to_string(),
            port: 8080,
            ..Default::default()
        };
        let config = AppConfig::merge(&cli, &FileConfig::default(), &DiscoveredConfig::default());
        assert_eq!(config.transport, Transport::Stdio);
        assert_eq!(config.port, 8080);
        assert!(config.local_repo_paths.is_empty());
        assert!(config.remote_repos.is_empty());
    }

    #[test]
    fn test_merge_cli_overrides_transport() {
        let cli = CliOverrides {
            transport: "sse".to_string(),
            port: 9090,
            ..Default::default()
        };
        let config = AppConfig::merge(&cli, &FileConfig::default(), &DiscoveredConfig::default());
        assert_eq!(config.transport, Transport::Sse);
        assert_eq!(config.port, 9090);
    }

    #[test]
    fn test_merge_lists_are_merged() {
        let cli = CliOverrides {
            transport: "stdio".to_string(),
            local_repos: vec!["/cli/repo".to_string()],
            remote_repos: vec!["https://cli.repo".to_string()],
            ..Default::default()
        };
        let discovered = DiscoveredConfig {
            local_repo_paths: vec![PathBuf::from("/discovered/repo")],
            remote_repos: vec![RemoteRepo {
                name: "discovered".to_string(),
                url: "https://discovered.repo".to_string(),
            }],
        };
        let file_config = FileConfig {
            local: Some(LocalFileConfig {
                extra_paths: Some(vec!["/file/repo".to_string()]),
            }),
            remote: Some(RemoteFileConfig {
                extra_repos: Some(vec![RemoteRepoEntry {
                    name: "file".to_string(),
                    url: "https://file.repo".to_string(),
                }]),
            }),
            ..Default::default()
        };
        let config = AppConfig::merge(&cli, &file_config, &discovered);
        assert_eq!(config.local_repo_paths.len(), 3);
        assert_eq!(config.remote_repos.len(), 3);
    }

    #[test]
    fn test_no_auto_discover_excludes_discovered() {
        let cli = CliOverrides {
            transport: "stdio".to_string(),
            no_auto_discover: true,
            local_repos: vec!["/cli/repo".to_string()],
            ..Default::default()
        };
        let discovered = DiscoveredConfig {
            local_repo_paths: vec![PathBuf::from("/discovered/repo")],
            ..Default::default()
        };
        let config = AppConfig::merge(&cli, &FileConfig::default(), &discovered);
        assert_eq!(config.local_repo_paths.len(), 1);
        assert_eq!(config.local_repo_paths[0], PathBuf::from("/cli/repo"));
    }

    #[test]
    fn test_cli_cache_dir_overrides_file() {
        let cli = CliOverrides {
            transport: "stdio".to_string(),
            cache_dir: Some("/cli/cache".to_string()),
            ..Default::default()
        };
        let file_config = FileConfig {
            storage: Some(StorageFileConfig {
                cache_dir: Some("/file/cache".to_string()),
            }),
            ..Default::default()
        };
        let config = AppConfig::merge(&cli, &file_config, &DiscoveredConfig::default());
        assert_eq!(config.cache_dir, PathBuf::from("/cli/cache"));
    }
}
