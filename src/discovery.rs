use crate::config::{DiscoveredConfig, RemoteRepo};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::path::Path;
use tracing::warn;

/// Main entry point: detect local Maven/Gradle caches and parse build files for remote repo URLs.
pub fn discover(working_dir: &Path) -> DiscoveredConfig {
    let mut config = DiscoveredConfig::default();

    // Detect local repository caches
    detect_local_repos(&mut config);

    // Parse ~/.m2/settings.xml for mirrors and repositories
    parse_maven_settings_file(&mut config);

    // Walk up from working_dir looking for pom.xml
    parse_pom_in_ancestors(working_dir, &mut config);

    // Parse build.gradle / build.gradle.kts in working_dir
    parse_gradle_in_dir(working_dir, &mut config);

    // Deduplicate remote repos by URL
    let mut seen = std::collections::HashSet::new();
    config.remote_repos.retain(|r| seen.insert(r.url.clone()));

    config
}

/// Detect well-known local repository cache directories.
fn detect_local_repos(config: &mut DiscoveredConfig) {
    if let Some(home) = dirs::home_dir() {
        let m2_repo = home.join(".m2").join("repository");
        if m2_repo.is_dir() {
            tracing::info!("Discovered Maven local repo: {}", m2_repo.display());
            config.local_repo_paths.push(m2_repo);
        }

        let gradle_cache = home
            .join(".gradle")
            .join("caches")
            .join("modules-2")
            .join("files-2.1");
        if gradle_cache.is_dir() {
            tracing::info!("Discovered Gradle cache: {}", gradle_cache.display());
            config.local_repo_paths.push(gradle_cache);
        }
    }
}

/// Parse ~/.m2/settings.xml and extract mirror URLs and repository URLs.
fn parse_maven_settings_file(config: &mut DiscoveredConfig) {
    let path = match dirs::home_dir() {
        Some(home) => home.join(".m2").join("settings.xml"),
        None => return,
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return, // File doesn't exist or unreadable — that's fine
    };

    match parse_maven_settings(&content) {
        Ok(repos) => config.remote_repos.extend(repos),
        Err(e) => warn!("Failed to parse {}: {}", path.display(), e),
    }
}

/// Parse Maven settings.xml content, returning discovered remote repos from mirrors and profiles.
fn parse_maven_settings(xml: &str) -> Result<Vec<RemoteRepo>, String> {
    let mut repos = Vec::new();
    let mut reader = Reader::from_str(xml);

    let mut tag_stack: Vec<String> = Vec::new();
    let mut current_mirror_url: Option<String> = None;
    let mut current_mirror_name: Option<String> = None;
    let mut current_repo_url: Option<String> = None;
    let mut current_repo_id: Option<String> = None;
    let mut in_mirror = false;
    let mut in_repository = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if name == "mirror" {
                    in_mirror = true;
                    current_mirror_url = None;
                    current_mirror_name = None;
                } else if name == "repository" {
                    in_repository = true;
                    current_repo_url = None;
                    current_repo_id = None;
                }
                tag_stack.push(name);
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if name == "mirror" {
                    if let Some(url) = current_mirror_url.take() {
                        repos.push(RemoteRepo {
                            name: current_mirror_name
                                .take()
                                .unwrap_or_else(|| "mirror".to_string()),
                            url,
                        });
                    }
                    in_mirror = false;
                } else if name == "repository" {
                    if let Some(url) = current_repo_url.take() {
                        repos.push(RemoteRepo {
                            name: current_repo_id
                                .take()
                                .unwrap_or_else(|| "repository".to_string()),
                            url,
                        });
                    }
                    in_repository = false;
                }
                tag_stack.pop();
            }
            Ok(Event::Text(e)) => {
                let text = e.unescape().unwrap_or_default().trim().to_string();
                if text.is_empty() {
                    continue;
                }
                let current_tag = tag_stack.last().map(|s| s.as_str()).unwrap_or("");
                if in_mirror {
                    match current_tag {
                        "url" => current_mirror_url = Some(text),
                        "name" => current_mirror_name = Some(text),
                        "id" => {
                            if current_mirror_name.is_none() {
                                current_mirror_name = Some(text);
                            }
                        }
                        _ => {}
                    }
                } else if in_repository {
                    match current_tag {
                        "url" => current_repo_url = Some(text),
                        "id" => current_repo_id = Some(text),
                        _ => {}
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML parse error: {}", e)),
            _ => {}
        }
    }

    Ok(repos)
}

/// Walk up from working_dir to find pom.xml files and parse repositories from them.
fn parse_pom_in_ancestors(working_dir: &Path, config: &mut DiscoveredConfig) {
    let mut dir = working_dir.to_path_buf();
    loop {
        let pom = dir.join("pom.xml");
        if pom.is_file() {
            match std::fs::read_to_string(&pom) {
                Ok(content) => match parse_pom_repositories(&content) {
                    Ok(repos) => config.remote_repos.extend(repos),
                    Err(e) => warn!("Failed to parse {}: {}", pom.display(), e),
                },
                Err(e) => warn!("Failed to read {}: {}", pom.display(), e),
            }
            break; // Only parse the nearest pom.xml
        }
        if !dir.pop() {
            break;
        }
    }
}

/// Parse a pom.xml string and extract repository URLs from <repositories> section.
fn parse_pom_repositories(xml: &str) -> Result<Vec<RemoteRepo>, String> {
    let mut repos = Vec::new();
    let mut reader = Reader::from_str(xml);

    let mut tag_stack: Vec<String> = Vec::new();
    let mut in_repositories = false;
    let mut in_repository = false;
    let mut current_url: Option<String> = None;
    let mut current_id: Option<String> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if name == "repositories" {
                    in_repositories = true;
                } else if in_repositories && name == "repository" {
                    in_repository = true;
                    current_url = None;
                    current_id = None;
                }
                tag_stack.push(name);
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if name == "repositories" {
                    in_repositories = false;
                } else if name == "repository" && in_repository {
                    if let Some(url) = current_url.take() {
                        repos.push(RemoteRepo {
                            name: current_id
                                .take()
                                .unwrap_or_else(|| "maven".to_string()),
                            url,
                        });
                    }
                    in_repository = false;
                }
                tag_stack.pop();
            }
            Ok(Event::Text(e)) => {
                let text = e.unescape().unwrap_or_default().trim().to_string();
                if text.is_empty() {
                    continue;
                }
                let current_tag = tag_stack.last().map(|s| s.as_str()).unwrap_or("");
                if in_repository {
                    match current_tag {
                        "url" => current_url = Some(text),
                        "id" => current_id = Some(text),
                        _ => {}
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML parse error: {}", e)),
            _ => {}
        }
    }

    Ok(repos)
}

/// Parse build.gradle or build.gradle.kts in the given directory for maven repository URLs.
fn parse_gradle_in_dir(dir: &Path, config: &mut DiscoveredConfig) {
    for filename in &["build.gradle", "build.gradle.kts"] {
        let path = dir.join(filename);
        if path.is_file() {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    let repos = extract_gradle_maven_urls(&content);
                    config.remote_repos.extend(repos);
                }
                Err(e) => warn!("Failed to read {}: {}", path.display(), e),
            }
        }
    }
}

/// Extract maven repository URLs from Gradle build file content.
/// Handles patterns like:
///   maven { url "https://..." }
///   maven { url = uri("https://...") }
///   maven("https://...")
///   maven { setUrl("https://...") }
fn extract_gradle_maven_urls(content: &str) -> Vec<RemoteRepo> {
    let mut repos = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Pattern: url "..." or url '...'
    for url in extract_quoted_after(content, "url") {
        if seen.insert(url.clone()) {
            repos.push(RemoteRepo {
                name: "gradle".to_string(),
                url,
            });
        }
    }

    // Pattern: uri("...") — typically url = uri("...")
    for url in extract_paren_quoted(content, "uri") {
        if seen.insert(url.clone()) {
            repos.push(RemoteRepo {
                name: "gradle".to_string(),
                url,
            });
        }
    }

    // Pattern: maven("...")
    for url in extract_paren_quoted(content, "maven") {
        if seen.insert(url.clone()) {
            repos.push(RemoteRepo {
                name: "gradle".to_string(),
                url,
            });
        }
    }

    // Pattern: setUrl("...")
    for url in extract_paren_quoted(content, "setUrl") {
        if seen.insert(url.clone()) {
            repos.push(RemoteRepo {
                name: "gradle".to_string(),
                url,
            });
        }
    }

    repos
}

/// Extract URLs from patterns like `keyword "url"` or `keyword 'url'`.
fn extract_quoted_after(content: &str, keyword: &str) -> Vec<String> {
    let mut results = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        // Find keyword followed by optional whitespace/= and a quoted string
        if let Some(pos) = trimmed.find(keyword) {
            let after = &trimmed[pos + keyword.len()..];
            let after = after.trim_start();
            // Skip '=' if present
            let after = if let Some(rest) = after.strip_prefix('=') {
                rest.trim_start()
            } else {
                after
            };
            if let Some(url) = extract_first_quoted_string(after) {
                if url.starts_with("http://") || url.starts_with("https://") {
                    results.push(url);
                }
            }
        }
    }
    results
}

/// Extract URLs from patterns like `func("url")` or `func('url')`.
fn extract_paren_quoted(content: &str, func_name: &str) -> Vec<String> {
    let mut results = Vec::new();
    let pattern = format!("{}(", func_name);
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(pos) = trimmed.find(&pattern) {
            let after = &trimmed[pos + pattern.len()..];
            if let Some(url) = extract_first_quoted_string(after) {
                if url.starts_with("http://") || url.starts_with("https://") {
                    results.push(url);
                }
            }
        }
    }
    results
}

/// Extract the first single- or double-quoted string from the given text.
fn extract_first_quoted_string(text: &str) -> Option<String> {
    let text = text.trim();
    for quote in ['"', '\''] {
        if text.starts_with(quote) {
            if let Some(end) = text[1..].find(quote) {
                return Some(text[1..1 + end].to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_maven_settings_mirrors() {
        let xml = r#"
        <settings>
          <mirrors>
            <mirror>
              <id>company-mirror</id>
              <name>Company Mirror</name>
              <url>https://repo.company.com/maven2</url>
              <mirrorOf>central</mirrorOf>
            </mirror>
          </mirrors>
        </settings>
        "#;

        let repos = parse_maven_settings(xml).unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].url, "https://repo.company.com/maven2");
        assert_eq!(repos[0].name, "Company Mirror");
    }

    #[test]
    fn test_parse_pom_repositories() {
        let xml = r#"
        <project>
          <repositories>
            <repository>
              <id>spring-releases</id>
              <url>https://repo.spring.io/release</url>
            </repository>
            <repository>
              <id>jboss-public</id>
              <url>https://repository.jboss.org/nexus/content/groups/public</url>
            </repository>
          </repositories>
        </project>
        "#;

        let repos = parse_pom_repositories(xml).unwrap();
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].name, "spring-releases");
        assert_eq!(repos[0].url, "https://repo.spring.io/release");
        assert_eq!(repos[1].name, "jboss-public");
        assert_eq!(
            repos[1].url,
            "https://repository.jboss.org/nexus/content/groups/public"
        );
    }

    #[test]
    fn test_parse_gradle_repositories() {
        let content = r#"
        repositories {
            mavenCentral()
            maven { url "https://plugins.gradle.org/m2/" }
            maven { url 'https://jitpack.io' }
        }
        "#;

        let repos = extract_gradle_maven_urls(content);
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].url, "https://plugins.gradle.org/m2/");
        assert_eq!(repos[1].url, "https://jitpack.io");
    }

    #[test]
    fn test_extract_gradle_url_variants() {
        // url = uri("...")
        let content = r#"maven { url = uri("https://maven.pkg.github.com/owner/repo") }"#;
        let repos = extract_gradle_maven_urls(content);
        assert!(repos.iter().any(|r| r.url == "https://maven.pkg.github.com/owner/repo"));

        // maven("...")
        let content = r#"maven("https://oss.sonatype.org/content/repositories/snapshots")"#;
        let repos = extract_gradle_maven_urls(content);
        assert!(repos.iter().any(|r| r.url == "https://oss.sonatype.org/content/repositories/snapshots"));

        // setUrl("...")
        let content = r#"maven { setUrl("https://dl.bintray.com/kotlin/kotlin-eap") }"#;
        let repos = extract_gradle_maven_urls(content);
        assert!(repos.iter().any(|r| r.url == "https://dl.bintray.com/kotlin/kotlin-eap"));

        // url = "..."
        let content = r#"maven { url = "https://example.com/repo" }"#;
        let repos = extract_gradle_maven_urls(content);
        assert!(repos.iter().any(|r| r.url == "https://example.com/repo"));
    }
}
