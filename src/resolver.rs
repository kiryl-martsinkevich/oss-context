use anyhow::{bail, Context, Result};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tracing::{debug, info};

use crate::config::{AppConfig, RemoteRepo};
use crate::store::LibraryId;

const MAVEN_CENTRAL_URL: &str = "https://repo1.maven.org/maven2";
const MAVEN_CENTRAL_NAME: &str = "central";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JarType {
    Javadoc,
    Sources,
}

impl JarType {
    fn classifier(&self) -> &'static str {
        match self {
            JarType::Javadoc => "javadoc",
            JarType::Sources => "sources",
        }
    }
}

pub struct ResolvedJar {
    pub jar_type: JarType,
    pub extracted_dir: TempDir,
}

/// Build the list of remotes, always appending Maven Central as a fallback
/// unless it is already present.
fn remotes_with_central(config_remotes: &[RemoteRepo]) -> Vec<RemoteRepo> {
    let mut remotes = config_remotes.to_vec();
    let has_central = remotes.iter().any(|r| {
        r.url.trim_end_matches('/') == MAVEN_CENTRAL_URL
    });
    if !has_central {
        remotes.push(RemoteRepo {
            name: MAVEN_CENTRAL_NAME.to_string(),
            url: MAVEN_CENTRAL_URL.to_string(),
        });
    }
    remotes
}

/// Resolve a library JAR (javadoc or sources) from local repos or remote repos.
///
/// Resolution chain:
/// 1. Local javadoc JAR
/// 2. Remote javadoc JAR
/// 3. Local sources JAR
/// 4. Remote sources JAR
/// 5. Error
pub fn resolve(lib: &LibraryId, config: &AppConfig) -> Result<ResolvedJar> {
    let remotes = remotes_with_central(&config.remote_repos);

    // Try javadoc first, then sources
    for jar_type in &[JarType::Javadoc, JarType::Sources] {
        // Try local repos
        if let Some(path) = find_local_jar(lib, &config.local_repo_paths, *jar_type) {
            info!(
                "Found local {} JAR for {}: {}",
                jar_type.classifier(),
                lib.to_string_id(),
                path.display()
            );
            let dir = extract_jar(&path)
                .with_context(|| format!("extracting local JAR {}", path.display()))?;
            return Ok(ResolvedJar {
                jar_type: *jar_type,
                extracted_dir: dir,
            });
        }

        // Try remote repos
        if let Some(resolved) = try_remote_download(lib, &remotes, *jar_type)? {
            return Ok(resolved);
        }
    }

    bail!(
        "Could not resolve javadoc or sources JAR for {}",
        lib.to_string_id()
    )
}

/// Search local Maven/Gradle caches for a JAR with the given classifier.
fn find_local_jar(
    lib: &LibraryId,
    local_paths: &[PathBuf],
    jar_type: JarType,
) -> Option<PathBuf> {
    let classifier = jar_type.classifier();
    let jar_name = format!(
        "{}-{}-{}.jar",
        lib.artifact_id, lib.version, classifier
    );

    for base in local_paths {
        // Maven layout: {base}/{group/as/path}/{artifact}/{version}/{jar}
        let maven_path = maven_jar_path(base, lib, &jar_name);
        debug!("Checking Maven path: {}", maven_path.display());
        if maven_path.is_file() {
            return Some(maven_path);
        }

        // Gradle layout: {base}/{group}/{artifact}/{version}/{hash}/{jar}
        // The hash directory is unknown, so we glob for it.
        let gradle_base = base
            .join(&lib.group_id)
            .join(&lib.artifact_id)
            .join(&lib.version);
        debug!("Checking Gradle base: {}", gradle_base.display());
        if gradle_base.is_dir() {
            if let Ok(entries) = fs::read_dir(&gradle_base) {
                for entry in entries.flatten() {
                    let candidate = entry.path().join(&jar_name);
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                }
            }
        }
    }

    None
}

/// Construct Maven-layout path for a JAR.
fn maven_jar_path(base: &Path, lib: &LibraryId, jar_name: &str) -> PathBuf {
    let group_path = lib.group_id.replace('.', "/");
    base.join(group_path)
        .join(&lib.artifact_id)
        .join(&lib.version)
        .join(jar_name)
}

/// Try downloading a JAR from each remote repo in order.
fn try_remote_download(
    lib: &LibraryId,
    remotes: &[RemoteRepo],
    jar_type: JarType,
) -> Result<Option<ResolvedJar>> {
    let classifier = jar_type.classifier();
    let group_path = lib.group_id.replace('.', "/");
    let jar_name = format!(
        "{}-{}-{}.jar",
        lib.artifact_id, lib.version, classifier
    );
    let relative = format!(
        "{}/{}/{}/{}",
        group_path, lib.artifact_id, lib.version, jar_name
    );

    for remote in remotes {
        let url = format!(
            "{}/{}",
            remote.url.trim_end_matches('/'),
            relative
        );
        debug!("Trying remote download: {}", url);

        match download_and_extract(&url) {
            Ok(dir) => {
                info!(
                    "Downloaded {} JAR for {} from {}",
                    classifier,
                    lib.to_string_id(),
                    remote.name
                );
                return Ok(Some(ResolvedJar {
                    jar_type,
                    extracted_dir: dir,
                }));
            }
            Err(e) => {
                debug!(
                    "Failed to download from {} ({}): {}",
                    remote.name, url, e
                );
            }
        }
    }

    Ok(None)
}

/// Download a JAR from a URL, save to a temp file, and extract it.
fn download_and_extract(url: &str) -> Result<TempDir> {
    let response = reqwest::blocking::get(url)?;
    if !response.status().is_success() {
        bail!("HTTP {}", response.status());
    }

    let tmp_file = tempfile::NamedTempFile::new()?;
    let bytes = response.bytes()?;
    fs::write(tmp_file.path(), &bytes)?;

    extract_jar(tmp_file.path())
}

/// Extract a JAR (zip) archive into a new temp directory.
fn extract_jar(jar_path: &Path) -> Result<TempDir> {
    let file = fs::File::open(jar_path)
        .with_context(|| format!("opening JAR {}", jar_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("reading ZIP {}", jar_path.display()))?;

    let dest = TempDir::new().context("creating temp dir for extraction")?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_string();

        // Skip directory entries and paths with suspicious components
        if name.ends_with('/') || name.contains("..") {
            continue;
        }

        let out_path = dest.path().join(&name);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut out_file = fs::File::create(&out_path)?;
        io::copy(&mut entry, &mut out_file)?;
    }

    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_remotes_with_central_adds_central() {
        let remotes = vec![RemoteRepo {
            name: "custom".to_string(),
            url: "https://custom.repo/maven".to_string(),
        }];
        let result = remotes_with_central(&remotes);
        assert_eq!(result.len(), 2);
        assert_eq!(result[1].name, MAVEN_CENTRAL_NAME);
        assert_eq!(result[1].url, MAVEN_CENTRAL_URL);
    }

    #[test]
    fn test_remotes_with_central_no_duplicate() {
        let remotes = vec![RemoteRepo {
            name: "central".to_string(),
            url: MAVEN_CENTRAL_URL.to_string(),
        }];
        let result = remotes_with_central(&remotes);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].url, MAVEN_CENTRAL_URL);
    }

    #[test]
    fn test_find_local_jar_not_found() {
        let lib = LibraryId {
            group_id: "com.example".to_string(),
            artifact_id: "foo".to_string(),
            version: "1.0.0".to_string(),
        };
        let paths = vec![PathBuf::from("/nonexistent/path")];
        let result = find_local_jar(&lib, &paths, JarType::Javadoc);
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_jar_invalid_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"not a zip file").unwrap();
        let result = extract_jar(tmp.path());
        assert!(result.is_err());
    }
}
