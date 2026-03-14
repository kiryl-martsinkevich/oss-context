# oss-context Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a Rust MCP server that provides Java library documentation on demand via stdio/SSE transport.

**Architecture:** Single binary with modules: config, discovery, store, resolver, parser (javadoc + source), tools, transport. SQLite+FTS5 for storage. tree-sitter for source parsing, scraper for javadoc HTML.

**Tech Stack:** Rust, rmcp, rusqlite (bundled+fts5), tree-sitter-java, scraper, reqwest, quick-xml, clap, tokio, tracing

---

### Task 1: Project Scaffolding

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `src/lib.rs`

**Step 1: Create Cargo.toml**

```toml
[package]
name = "oss-context"
version = "0.1.0"
edition = "2021"

[dependencies]
rmcp = { version = "0.16", features = ["server", "transport-io", "transport-sse-server"] }
rusqlite = { version = "0.34", features = ["bundled"] }
tree-sitter = "0.26"
tree-sitter-java = "0.23"
scraper = "0.22"
reqwest = { version = "0.12", features = ["blocking"] }
quick-xml = { version = "0.37", features = ["serialize"] }
zip = "2.6"
toml = "0.8"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
schemars = "0.8"
clap = { version = "4", features = ["derive"] }
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
tempfile = "3"
dirs = "6"
anyhow = "1"
thiserror = "2"
```

**Step 2: Create src/lib.rs with module declarations**

```rust
pub mod config;
pub mod discovery;
pub mod store;
pub mod resolver;
pub mod parser;
pub mod tools;
pub mod transport;
```

**Step 3: Create src/main.rs with minimal entrypoint**

```rust
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
```

**Step 4: Create stub modules**

Create empty files: `src/config.rs`, `src/discovery.rs`, `src/store.rs`, `src/resolver.rs`, `src/parser.rs`, `src/tools.rs`, `src/transport.rs`

**Step 5: Verify it compiles**

Run: `cargo build`
Expected: Compiles with warnings about unused modules

**Step 6: Commit**

```bash
git add -A && git commit -m "feat: project scaffolding with dependencies and CLI"
```

---

### Task 2: Config Module

**Files:**
- Create: `src/config.rs`
- Test: inline `#[cfg(test)]` module

**Step 1: Write tests for config merging**

```rust
// src/config.rs
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
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            toml::from_str(&content).unwrap_or_default()
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
```

Note: also add `shellexpand = "3"` to Cargo.toml dependencies.

**Step 2: Run tests**

Run: `cargo test config`
Expected: All 5 tests pass

**Step 3: Commit**

```bash
git add -A && git commit -m "feat: config module with layered merging (CLI > file > discovered)"
```

---

### Task 3: Store Module (SQLite + FTS5)

**Files:**
- Create: `src/store.rs`

**Step 1: Write the store with schema creation and CRUD**

```rust
// src/store.rs
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct LibraryId {
    pub group_id: String,
    pub artifact_id: String,
    pub version: String,
}

impl LibraryId {
    pub fn to_string_id(&self) -> String {
        format!("{}:{}:{}", self.group_id, self.artifact_id, self.version)
    }

    pub fn db_path(&self, cache_dir: &Path) -> PathBuf {
        cache_dir
            .join(&self.group_id)
            .join(&self.artifact_id)
            .join(format!("{}.db", self.version))
    }
}

pub struct Store {
    conn: Connection,
}

#[derive(Debug, Clone)]
pub struct PackageRow {
    pub id: i64,
    pub name: String,
    pub doc_comment: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TypeRow {
    pub id: i64,
    pub package_id: i64,
    pub name: String,
    pub fqn: String,
    pub kind: String,
    pub doc_comment: Option<String>,
    pub annotations: Option<String>,
    pub superclass: Option<String>,
    pub interfaces: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MethodRow {
    pub id: i64,
    pub type_id: i64,
    pub name: String,
    pub signature: String,
    pub return_type: Option<String>,
    pub params: String,
    pub doc_comment: Option<String>,
    pub annotations: Option<String>,
    pub is_static: bool,
}

#[derive(Debug, Clone)]
pub struct FieldRow {
    pub id: i64,
    pub type_id: i64,
    pub name: String,
    pub field_type: String,
    pub doc_comment: Option<String>,
    pub annotations: Option<String>,
    pub is_static: bool,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub fqn: String,
    pub kind: String,
    pub signature: Option<String>,
    pub doc_comment: Option<String>,
    pub rank: f64,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.create_schema()?;
        Ok(store)
    }

    pub fn open_if_exists(path: &Path) -> Result<Option<Self>> {
        if path.exists() {
            Ok(Some(Self::open(path)?))
        } else {
            Ok(None)
        }
    }

    fn create_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS library (
                group_id    TEXT NOT NULL,
                artifact_id TEXT NOT NULL,
                version     TEXT NOT NULL,
                source_type TEXT NOT NULL,
                indexed_at  TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS package (
                id          INTEGER PRIMARY KEY,
                name        TEXT NOT NULL UNIQUE,
                doc_comment TEXT
            );

            CREATE TABLE IF NOT EXISTS type (
                id          INTEGER PRIMARY KEY,
                package_id  INTEGER NOT NULL REFERENCES package(id),
                name        TEXT NOT NULL,
                fqn         TEXT NOT NULL UNIQUE,
                kind        TEXT NOT NULL,
                doc_comment TEXT,
                annotations TEXT,
                superclass  TEXT,
                interfaces  TEXT
            );

            CREATE TABLE IF NOT EXISTS method (
                id          INTEGER PRIMARY KEY,
                type_id     INTEGER NOT NULL REFERENCES type(id),
                name        TEXT NOT NULL,
                signature   TEXT NOT NULL,
                return_type TEXT,
                params      TEXT NOT NULL,
                doc_comment TEXT,
                annotations TEXT,
                is_static   INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS field (
                id          INTEGER PRIMARY KEY,
                type_id     INTEGER NOT NULL REFERENCES type(id),
                name        TEXT NOT NULL,
                field_type  TEXT NOT NULL,
                doc_comment TEXT,
                annotations TEXT,
                is_static   INTEGER NOT NULL DEFAULT 0
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS docs_fts USING fts5(
                fqn,
                kind,
                signature,
                doc_comment
            );

            CREATE INDEX IF NOT EXISTS idx_type_package ON type(package_id);
            CREATE INDEX IF NOT EXISTS idx_method_type ON method(type_id);
            CREATE INDEX IF NOT EXISTS idx_field_type ON field(type_id);
            "
        )?;
        Ok(())
    }

    pub fn set_library_meta(&self, lib: &LibraryId, source_type: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO library (group_id, artifact_id, version, source_type, indexed_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))",
            params![lib.group_id, lib.artifact_id, lib.version, source_type],
        )?;
        Ok(())
    }

    pub fn has_library_meta(&self) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM library",
            [],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn insert_package(&self, name: &str, doc_comment: Option<&str>) -> Result<i64> {
        self.conn.execute(
            "INSERT OR IGNORE INTO package (name, doc_comment) VALUES (?1, ?2)",
            params![name, doc_comment],
        )?;
        let id = self.conn.query_row(
            "SELECT id FROM package WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    pub fn insert_type(&self, row: &TypeRow) -> Result<i64> {
        self.conn.execute(
            "INSERT OR REPLACE INTO type (package_id, name, fqn, kind, doc_comment, annotations, superclass, interfaces)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                row.package_id, row.name, row.fqn, row.kind,
                row.doc_comment, row.annotations, row.superclass, row.interfaces
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        // Index in FTS
        self.conn.execute(
            "INSERT INTO docs_fts (fqn, kind, signature, doc_comment) VALUES (?1, ?2, ?3, ?4)",
            params![row.fqn, row.kind, "", row.doc_comment],
        )?;
        Ok(id)
    }

    pub fn insert_method(&self, row: &MethodRow) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO method (type_id, name, signature, return_type, params, doc_comment, annotations, is_static)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                row.type_id, row.name, row.signature, row.return_type,
                row.params, row.doc_comment, row.annotations, row.is_static as i32
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        // Get parent FQN for method FQN
        let parent_fqn: String = self.conn.query_row(
            "SELECT fqn FROM type WHERE id = ?1",
            params![row.type_id],
            |r| r.get(0),
        )?;
        let method_fqn = format!("{}.{}", parent_fqn, row.name);
        self.conn.execute(
            "INSERT INTO docs_fts (fqn, kind, signature, doc_comment) VALUES (?1, ?2, ?3, ?4)",
            params![method_fqn, "method", row.signature, row.doc_comment],
        )?;
        Ok(id)
    }

    pub fn insert_field(&self, row: &FieldRow) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO field (type_id, name, field_type, doc_comment, annotations, is_static)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                row.type_id, row.name, row.field_type,
                row.doc_comment, row.annotations, row.is_static as i32
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        let parent_fqn: String = self.conn.query_row(
            "SELECT fqn FROM type WHERE id = ?1",
            params![row.type_id],
            |r| r.get(0),
        )?;
        let field_fqn = format!("{}.{}", parent_fqn, row.name);
        self.conn.execute(
            "INSERT INTO docs_fts (fqn, kind, signature, doc_comment) VALUES (?1, ?2, ?3, ?4)",
            params![field_fqn, "field", row.field_type, row.doc_comment],
        )?;
        Ok(id)
    }

    /// Full-text search across all docs
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let mut stmt = self.conn.prepare(
            "SELECT fqn, kind, signature, doc_comment, bm25(docs_fts, 10.0, 5.0, 3.0, 1.0) as rank
             FROM docs_fts
             WHERE docs_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2"
        )?;
        let results = stmt.query_map(params![query, limit as i64], |row| {
            Ok(SearchResult {
                fqn: row.get(0)?,
                kind: row.get(1)?,
                signature: row.get(2)?,
                doc_comment: row.get(3)?,
                rank: row.get(4)?,
            })
        })?;
        results.into_iter().map(|r| r.map_err(Into::into)).collect()
    }

    /// List all packages
    pub fn list_packages(&self) -> Result<Vec<PackageRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, doc_comment FROM package ORDER BY name"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(PackageRow {
                id: row.get(0)?,
                name: row.get(1)?,
                doc_comment: row.get(2)?,
            })
        })?;
        rows.into_iter().map(|r| r.map_err(Into::into)).collect()
    }

    /// List types in a package
    pub fn list_types_in_package(&self, package_name: &str) -> Result<Vec<TypeRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.package_id, t.name, t.fqn, t.kind, t.doc_comment, t.annotations, t.superclass, t.interfaces
             FROM type t
             JOIN package p ON t.package_id = p.id
             WHERE p.name = ?1
             ORDER BY t.name"
        )?;
        let rows = stmt.query_map(params![package_name], |row| {
            Ok(TypeRow {
                id: row.get(0)?,
                package_id: row.get(1)?,
                name: row.get(2)?,
                fqn: row.get(3)?,
                kind: row.get(4)?,
                doc_comment: row.get(5)?,
                annotations: row.get(6)?,
                superclass: row.get(7)?,
                interfaces: row.get(8)?,
            })
        })?;
        rows.into_iter().map(|r| r.map_err(Into::into)).collect()
    }

    /// Get full type details by FQN
    pub fn get_type_by_fqn(&self, fqn: &str) -> Result<Option<TypeRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, package_id, name, fqn, kind, doc_comment, annotations, superclass, interfaces
             FROM type WHERE fqn = ?1"
        )?;
        let mut rows = stmt.query_map(params![fqn], |row| {
            Ok(TypeRow {
                id: row.get(0)?,
                package_id: row.get(1)?,
                name: row.get(2)?,
                fqn: row.get(3)?,
                kind: row.get(4)?,
                doc_comment: row.get(5)?,
                annotations: row.get(6)?,
                superclass: row.get(7)?,
                interfaces: row.get(8)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// Get methods for a type
    pub fn get_methods_for_type(&self, type_id: i64) -> Result<Vec<MethodRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, type_id, name, signature, return_type, params, doc_comment, annotations, is_static
             FROM method WHERE type_id = ?1 ORDER BY name"
        )?;
        let rows = stmt.query_map(params![type_id], |row| {
            Ok(MethodRow {
                id: row.get(0)?,
                type_id: row.get(1)?,
                name: row.get(2)?,
                signature: row.get(3)?,
                return_type: row.get(4)?,
                params: row.get(5)?,
                doc_comment: row.get(6)?,
                annotations: row.get(7)?,
                is_static: row.get::<_, i32>(8)? != 0,
            })
        })?;
        rows.into_iter().map(|r| r.map_err(Into::into)).collect()
    }

    /// Get fields for a type
    pub fn get_fields_for_type(&self, type_id: i64) -> Result<Vec<FieldRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, type_id, name, field_type, doc_comment, annotations, is_static
             FROM field WHERE type_id = ?1 ORDER BY name"
        )?;
        let rows = stmt.query_map(params![type_id], |row| {
            Ok(FieldRow {
                id: row.get(0)?,
                type_id: row.get(1)?,
                name: row.get(2)?,
                field_type: row.get(3)?,
                doc_comment: row.get(4)?,
                annotations: row.get(5)?,
                is_static: row.get::<_, i32>(6)? != 0,
            })
        })?;
        rows.into_iter().map(|r| r.map_err(Into::into)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn test_store() -> Store {
        let tmp = NamedTempFile::new().unwrap();
        Store::open(tmp.path()).unwrap()
    }

    #[test]
    fn test_schema_creation() {
        let store = test_store();
        assert!(!store.has_library_meta().unwrap());
    }

    #[test]
    fn test_insert_and_query_package() {
        let store = test_store();
        let pkg_id = store.insert_package("com.example", Some("Example package")).unwrap();
        let packages = store.list_packages().unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "com.example");
        assert_eq!(packages[0].id, pkg_id);
    }

    #[test]
    fn test_insert_type_and_search() {
        let store = test_store();
        let pkg_id = store.insert_package("com.example", None).unwrap();
        let type_row = TypeRow {
            id: 0,
            package_id: pkg_id,
            name: "MyClass".to_string(),
            fqn: "com.example.MyClass".to_string(),
            kind: "class".to_string(),
            doc_comment: Some("A sample class for testing".to_string()),
            annotations: None,
            superclass: None,
            interfaces: None,
        };
        store.insert_type(&type_row).unwrap();

        let results = store.search("sample class", 10).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].fqn, "com.example.MyClass");
    }

    #[test]
    fn test_insert_method_and_browse() {
        let store = test_store();
        let pkg_id = store.insert_package("com.example", None).unwrap();
        let type_row = TypeRow {
            id: 0, package_id: pkg_id, name: "MyClass".to_string(),
            fqn: "com.example.MyClass".to_string(), kind: "class".to_string(),
            doc_comment: None, annotations: None, superclass: None, interfaces: None,
        };
        let type_id = store.insert_type(&type_row).unwrap();
        let method_row = MethodRow {
            id: 0, type_id, name: "doSomething".to_string(),
            signature: "public void doSomething(String input)".to_string(),
            return_type: Some("void".to_string()),
            params: r#"[{"name":"input","type":"String"}]"#.to_string(),
            doc_comment: Some("Does something useful".to_string()),
            annotations: None, is_static: false,
        };
        store.insert_method(&method_row).unwrap();

        let methods = store.get_methods_for_type(type_id).unwrap();
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].name, "doSomething");

        let results = store.search("something useful", 10).unwrap();
        assert!(!results.is_empty());
    }

    #[test]
    fn test_browse_type_by_fqn() {
        let store = test_store();
        let pkg_id = store.insert_package("com.example", None).unwrap();
        let type_row = TypeRow {
            id: 0, package_id: pkg_id, name: "Foo".to_string(),
            fqn: "com.example.Foo".to_string(), kind: "interface".to_string(),
            doc_comment: Some("A foo interface".to_string()), annotations: None,
            superclass: None, interfaces: None,
        };
        store.insert_type(&type_row).unwrap();

        let found = store.get_type_by_fqn("com.example.Foo").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().kind, "interface");

        let not_found = store.get_type_by_fqn("com.example.Bar").unwrap();
        assert!(not_found.is_none());
    }
}
```

**Step 2: Run tests**

Run: `cargo test store`
Expected: All 5 tests pass

**Step 3: Commit**

```bash
git add -A && git commit -m "feat: SQLite store with FTS5 search, CRUD, and browse queries"
```

---

### Task 4: Discovery Module

**Files:**
- Create: `src/discovery.rs`

**Step 1: Implement discovery for local repos and settings.xml/pom.xml/build.gradle**

```rust
// src/discovery.rs
use crate::config::{DiscoveredConfig, RemoteRepo};
use anyhow::Result;
use std::path::{Path, PathBuf};
use tracing::warn;

/// Run all auto-discovery and return merged results
pub fn discover(working_dir: &Path) -> DiscoveredConfig {
    let mut config = DiscoveredConfig::default();

    // Discover local repo paths
    discover_local_repos(&mut config);

    // Discover remote repos from settings.xml
    if let Err(e) = discover_from_maven_settings(&mut config) {
        warn!("Failed to parse Maven settings.xml: {}", e);
    }

    // Discover remote repos from pom.xml
    if let Err(e) = discover_from_pom(working_dir, &mut config) {
        warn!("Failed to parse pom.xml: {}", e);
    }

    // Discover remote repos from build.gradle
    if let Err(e) = discover_from_gradle(working_dir, &mut config) {
        warn!("Failed to parse build.gradle: {}", e);
    }

    config
}

fn discover_local_repos(config: &mut DiscoveredConfig) {
    // Maven local repo
    let m2_repo = dirs::home_dir()
        .map(|h| h.join(".m2").join("repository"));
    if let Some(ref path) = m2_repo {
        if path.exists() {
            config.local_repo_paths.push(path.clone());
        }
    }

    // Gradle cache
    let gradle_cache = dirs::home_dir()
        .map(|h| h.join(".gradle").join("caches").join("modules-2").join("files-2.1"));
    if let Some(ref path) = gradle_cache {
        if path.exists() {
            config.local_repo_paths.push(path.clone());
        }
    }
}

fn discover_from_maven_settings(config: &mut DiscoveredConfig) -> Result<()> {
    let settings_path = dirs::home_dir()
        .map(|h| h.join(".m2").join("settings.xml"));
    let path = match settings_path {
        Some(p) if p.exists() => p,
        _ => return Ok(()),
    };

    let content = std::fs::read_to_string(&path)?;
    parse_maven_settings(&content, config);
    Ok(())
}

fn parse_maven_settings(xml: &str, config: &mut DiscoveredConfig) {
    // Simple extraction of <url> inside <repository> or <mirror> elements
    // Using quick-xml for proper parsing
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut in_mirror = false;
    let mut in_repository = false;
    let mut in_url = false;
    let mut in_local_repository = false;
    let mut current_url = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match name.as_str() {
                    "mirror" => in_mirror = true,
                    "repository" => in_repository = true,
                    "url" if in_mirror || in_repository => in_url = true,
                    "localRepository" => in_local_repository = true,
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match name.as_str() {
                    "mirror" => {
                        if !current_url.is_empty() {
                            config.remote_repos.push(RemoteRepo {
                                name: "maven-mirror".to_string(),
                                url: current_url.clone(),
                            });
                            current_url.clear();
                        }
                        in_mirror = false;
                    }
                    "repository" => {
                        if !current_url.is_empty() {
                            config.remote_repos.push(RemoteRepo {
                                name: "maven-settings".to_string(),
                                url: current_url.clone(),
                            });
                            current_url.clear();
                        }
                        in_repository = false;
                    }
                    "url" => in_url = false,
                    "localRepository" => in_local_repository = false,
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                if in_url {
                    current_url = e.unescape().unwrap_or_default().trim().to_string();
                }
                if in_local_repository {
                    let path_str = e.unescape().unwrap_or_default().trim().to_string();
                    let path = PathBuf::from(shellexpand::tilde(&path_str).as_ref());
                    if path.exists() {
                        // Insert at front since settings.xml local repo overrides default
                        config.local_repo_paths.insert(0, path);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                warn!("XML parse error in settings.xml: {}", e);
                break;
            }
            _ => {}
        }
        buf.clear();
    }
}

fn discover_from_pom(working_dir: &Path, config: &mut DiscoveredConfig) -> Result<()> {
    // Walk up from working_dir looking for pom.xml
    let mut dir = working_dir.to_path_buf();
    loop {
        let pom_path = dir.join("pom.xml");
        if pom_path.exists() {
            let content = std::fs::read_to_string(&pom_path)?;
            parse_pom_repositories(&content, config);
            break;
        }
        if !dir.pop() {
            break;
        }
    }
    Ok(())
}

fn parse_pom_repositories(xml: &str, config: &mut DiscoveredConfig) {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut in_repositories = false;
    let mut in_repository = false;
    let mut in_url = false;
    let mut depth = 0;
    let mut current_url = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match name.as_str() {
                    "repositories" => { in_repositories = true; depth = 0; }
                    "repository" if in_repositories => { in_repository = true; }
                    "url" if in_repository => { in_url = true; }
                    _ => {}
                }
                if in_repositories { depth += 1; }
            }
            Ok(Event::End(ref e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if in_repositories { depth -= 1; }
                match name.as_str() {
                    "repositories" => { in_repositories = false; }
                    "repository" if in_repository => {
                        if !current_url.is_empty() {
                            config.remote_repos.push(RemoteRepo {
                                name: "pom".to_string(),
                                url: current_url.clone(),
                            });
                            current_url.clear();
                        }
                        in_repository = false;
                    }
                    "url" => { in_url = false; }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                if in_url {
                    current_url = e.unescape().unwrap_or_default().trim().to_string();
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
}

fn discover_from_gradle(working_dir: &Path, config: &mut DiscoveredConfig) -> Result<()> {
    for name in &["build.gradle", "build.gradle.kts"] {
        let path = working_dir.join(name);
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            parse_gradle_repositories(&content, config);
            return Ok(());
        }
    }
    Ok(())
}

fn parse_gradle_repositories(content: &str, config: &mut DiscoveredConfig) {
    // Simple regex-like extraction of maven { url ... } patterns
    // Handles: maven { url "https://..." } and maven("https://...")
    // and url = uri("https://...")
    for line in content.lines() {
        let trimmed = line.trim();
        // Match: url "https://..." or url 'https://...' or url = uri("https://...")
        if let Some(url) = extract_gradle_url(trimmed) {
            if url.starts_with("http") {
                config.remote_repos.push(RemoteRepo {
                    name: "gradle".to_string(),
                    url,
                });
            }
        }
    }
}

fn extract_gradle_url(line: &str) -> Option<String> {
    // Pattern: url "..." or url '...'
    if let Some(rest) = line.strip_prefix("url") {
        let rest = rest.trim().trim_start_matches('=').trim();
        return extract_quoted_string(rest);
    }
    // Pattern: maven("...")
    if let Some(rest) = line.strip_prefix("maven(") {
        return extract_quoted_string(rest);
    }
    // Pattern: url = uri("...")
    if line.contains("uri(") {
        if let Some(start) = line.find("uri(") {
            return extract_quoted_string(&line[start + 4..]);
        }
    }
    None
}

fn extract_quoted_string(s: &str) -> Option<String> {
    let s = s.trim();
    if s.starts_with('"') {
        let end = s[1..].find('"')?;
        Some(s[1..1 + end].to_string())
    } else if s.starts_with('\'') {
        let end = s[1..].find('\'')?;
        Some(s[1..1 + end].to_string())
    } else {
        None
    }
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
                    <url>https://mirror.example.com/maven2</url>
                </mirror>
            </mirrors>
        </settings>
        "#;
        let mut config = DiscoveredConfig::default();
        parse_maven_settings(xml, &mut config);
        assert_eq!(config.remote_repos.len(), 1);
        assert_eq!(config.remote_repos[0].url, "https://mirror.example.com/maven2");
    }

    #[test]
    fn test_parse_pom_repositories() {
        let xml = r#"
        <project>
            <repositories>
                <repository>
                    <id>central</id>
                    <url>https://repo.example.com/maven2</url>
                </repository>
                <repository>
                    <id>snapshots</id>
                    <url>https://snapshots.example.com/maven2</url>
                </repository>
            </repositories>
        </project>
        "#;
        let mut config = DiscoveredConfig::default();
        parse_pom_repositories(xml, &mut config);
        assert_eq!(config.remote_repos.len(), 2);
    }

    #[test]
    fn test_parse_gradle_repositories() {
        let content = r#"
        repositories {
            mavenCentral()
            maven { url "https://jitpack.io" }
            maven { url 'https://plugins.gradle.org/m2/' }
        }
        "#;
        let mut config = DiscoveredConfig::default();
        parse_gradle_repositories(content, &mut config);
        assert_eq!(config.remote_repos.len(), 2);
        assert_eq!(config.remote_repos[0].url, "https://jitpack.io");
    }

    #[test]
    fn test_extract_gradle_url_variants() {
        assert_eq!(extract_gradle_url(r#"url "https://example.com""#), Some("https://example.com".into()));
        assert_eq!(extract_gradle_url(r#"url 'https://example.com'"#), Some("https://example.com".into()));
        assert_eq!(extract_gradle_url(r#"maven("https://example.com")"#), Some("https://example.com".into()));
        assert_eq!(extract_gradle_url("something else"), None);
    }
}
```

**Step 2: Run tests**

Run: `cargo test discovery`
Expected: All 4 tests pass

**Step 3: Commit**

```bash
git add -A && git commit -m "feat: auto-discovery of local/remote Maven and Gradle repos"
```

---

### Task 5: Resolver Module

**Files:**
- Create: `src/resolver.rs`

**Step 1: Implement JAR resolution chain**

```rust
// src/resolver.rs
use crate::config::{AppConfig, RemoteRepo};
use crate::store::LibraryId;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tracing::{info, warn};

#[derive(Debug)]
pub enum JarType {
    Javadoc,
    Sources,
}

#[derive(Debug)]
pub struct ResolvedJar {
    pub jar_type: JarType,
    pub extracted_dir: TempDir,
}

const MAVEN_CENTRAL: &str = "https://repo1.maven.org/maven2";

/// Resolve a library's documentation JAR through the resolution chain.
/// Returns the extracted contents in a temp directory.
pub fn resolve(lib: &LibraryId, config: &AppConfig) -> Result<ResolvedJar> {
    // 1. Javadoc JAR local
    if let Some(jar) = find_local_jar(lib, &config.local_repo_paths, "javadoc")? {
        info!("Found local javadoc JAR: {}", jar.display());
        let dir = extract_jar(&jar)?;
        return Ok(ResolvedJar { jar_type: JarType::Javadoc, extracted_dir: dir });
    }

    // 2. Javadoc JAR remote
    let remotes = remotes_with_central(&config.remote_repos);
    if let Some(dir) = try_download_jar(lib, &remotes, "javadoc")? {
        return Ok(ResolvedJar { jar_type: JarType::Javadoc, extracted_dir: dir });
    }

    // 3. Sources JAR local
    if let Some(jar) = find_local_jar(lib, &config.local_repo_paths, "sources")? {
        info!("Found local sources JAR: {}", jar.display());
        let dir = extract_jar(&jar)?;
        return Ok(ResolvedJar { jar_type: JarType::Sources, extracted_dir: dir });
    }

    // 4. Sources JAR remote
    if let Some(dir) = try_download_jar(lib, &remotes, "sources")? {
        return Ok(ResolvedJar { jar_type: JarType::Sources, extracted_dir: dir });
    }

    bail!(
        "Could not find javadoc or sources JAR for {}:{}:{}",
        lib.group_id, lib.artifact_id, lib.version
    )
}

fn remotes_with_central(configured: &[RemoteRepo]) -> Vec<RemoteRepo> {
    let mut repos = configured.to_vec();
    // Add Maven Central as fallback if not already present
    let has_central = repos.iter().any(|r| r.url.contains("repo1.maven.org"));
    if !has_central {
        repos.push(RemoteRepo {
            name: "central".to_string(),
            url: MAVEN_CENTRAL.to_string(),
        });
    }
    repos
}

fn find_local_jar(lib: &LibraryId, local_paths: &[PathBuf], classifier: &str) -> Result<Option<PathBuf>> {
    for base in local_paths {
        // Maven layout: {base}/{g/as/path}/{a}/{v}/{a}-{v}-{classifier}.jar
        let group_path = lib.group_id.replace('.', "/");
        let jar_name = format!("{}-{}-{}.jar", lib.artifact_id, lib.version, classifier);
        let maven_path = base
            .join(&group_path)
            .join(&lib.artifact_id)
            .join(&lib.version)
            .join(&jar_name);
        if maven_path.exists() {
            return Ok(Some(maven_path));
        }

        // Gradle layout: {base}/{g}/{a}/{v}/{hash}/{a}-{v}-{classifier}.jar
        // Gradle uses hash subdirectories, so we need to glob
        let gradle_dir = base
            .join(&lib.group_id)
            .join(&lib.artifact_id)
            .join(&lib.version);
        if gradle_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&gradle_dir) {
                for entry in entries.flatten() {
                    let sub = entry.path();
                    if sub.is_dir() {
                        let candidate = sub.join(&jar_name);
                        if candidate.exists() {
                            return Ok(Some(candidate));
                        }
                    }
                }
            }
        }
    }
    Ok(None)
}

fn try_download_jar(lib: &LibraryId, remotes: &[RemoteRepo], classifier: &str) -> Result<Option<TempDir>> {
    let group_path = lib.group_id.replace('.', "/");
    let jar_name = format!("{}-{}-{}.jar", lib.artifact_id, lib.version, classifier);

    for repo in remotes {
        let url = format!(
            "{}/{}/{}/{}/{}",
            repo.url.trim_end_matches('/'),
            group_path,
            lib.artifact_id,
            lib.version,
            jar_name
        );
        info!("Trying remote: {}", url);
        match download_and_extract(&url) {
            Ok(dir) => {
                info!("Downloaded {} JAR from {}", classifier, repo.name);
                return Ok(Some(dir));
            }
            Err(e) => {
                warn!("Failed to download from {}: {}", url, e);
                continue;
            }
        }
    }
    Ok(None)
}

fn download_and_extract(url: &str) -> Result<TempDir> {
    let response = reqwest::blocking::get(url)?;
    if !response.status().is_success() {
        bail!("HTTP {}", response.status());
    }
    let bytes = response.bytes()?;

    let tmp_jar = tempfile::NamedTempFile::new()?;
    std::fs::write(tmp_jar.path(), &bytes)?;

    extract_jar(tmp_jar.path())
}

fn extract_jar(jar_path: &Path) -> Result<TempDir> {
    let file = std::fs::File::open(jar_path)?;
    let mut archive = zip::ZipArchive::new(file)
        .context("Failed to open JAR as ZIP archive")?;

    let dir = TempDir::new()?;
    archive.extract(dir.path())?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remotes_with_central_adds_central() {
        let repos = remotes_with_central(&[]);
        assert_eq!(repos.len(), 1);
        assert!(repos[0].url.contains("repo1.maven.org"));
    }

    #[test]
    fn test_remotes_with_central_no_duplicate() {
        let repos = remotes_with_central(&[RemoteRepo {
            name: "central".to_string(),
            url: "https://repo1.maven.org/maven2".to_string(),
        }]);
        assert_eq!(repos.len(), 1);
    }

    #[test]
    fn test_find_local_jar_not_found() {
        let lib = LibraryId {
            group_id: "com.example".into(),
            artifact_id: "foo".into(),
            version: "1.0".into(),
        };
        let result = find_local_jar(&lib, &[PathBuf::from("/nonexistent")], "javadoc").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_jar_invalid_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"not a zip").unwrap();
        assert!(extract_jar(tmp.path()).is_err());
    }
}
```

**Step 2: Run tests**

Run: `cargo test resolver`
Expected: All 4 tests pass

**Step 3: Commit**

```bash
git add -A && git commit -m "feat: JAR resolver with local/remote resolution chain"
```

---

### Task 6: Source Parser (tree-sitter)

**Files:**
- Create: `src/parser/mod.rs`
- Create: `src/parser/source.rs`
- Create: `src/parser/javadoc.rs`

Update `src/lib.rs` — change `pub mod parser;` (it now expects a directory module).

**Step 1: Create parser module structure**

```rust
// src/parser/mod.rs
pub mod source;
pub mod javadoc;

use crate::resolver::{JarType, ResolvedJar};
use crate::store::{Store, LibraryId};
use anyhow::Result;

/// Parse a resolved JAR and populate the store
pub fn index_jar(jar: &ResolvedJar, lib: &LibraryId, store: &Store) -> Result<()> {
    match jar.jar_type {
        JarType::Javadoc => {
            javadoc::parse_javadoc_dir(jar.extracted_dir.path(), store)?;
            store.set_library_meta(lib, "javadoc")?;
        }
        JarType::Sources => {
            source::parse_source_dir(jar.extracted_dir.path(), store)?;
            store.set_library_meta(lib, "source")?;
        }
    }
    Ok(())
}
```

**Step 2: Implement source parser**

```rust
// src/parser/source.rs
use crate::store::{FieldRow, MethodRow, Store, TypeRow};
use anyhow::Result;
use std::path::Path;
use tracing::warn;

pub fn parse_source_dir(dir: &Path, store: &Store) -> Result<()> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_java::LANGUAGE.into())
        .expect("Failed to load Java grammar");

    walk_java_files(dir, &mut |path| {
        let content = std::fs::read_to_string(path)?;
        parse_java_file(&mut parser, &content, store)?;
        Ok(())
    })?;
    Ok(())
}

fn walk_java_files(dir: &Path, handler: &mut dyn FnMut(&Path) -> Result<()>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_java_files(&path, handler)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("java") {
            if let Err(e) = handler(&path) {
                warn!("Failed to parse {}: {}", path.display(), e);
            }
        }
    }
    Ok(())
}

fn parse_java_file(parser: &mut tree_sitter::Parser, source: &str, store: &Store) -> Result<()> {
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("Failed to parse Java source"))?;

    let root = tree.root_node();

    // Extract package name
    let package_name = extract_package_name(root, source).unwrap_or_default();
    let pkg_id = if !package_name.is_empty() {
        store.insert_package(&package_name, None)?
    } else {
        store.insert_package("(default)", None)?
    };

    // Extract type declarations
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "class_declaration" | "interface_declaration" | "enum_declaration"
            | "annotation_type_declaration" | "record_declaration" => {
                parse_type_declaration(child, source, &package_name, pkg_id, store)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn extract_package_name(root: tree_sitter::Node, source: &str) -> Option<String> {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "package_declaration" {
            // Find the scoped_identifier or identifier child
            let mut inner = child.walk();
            for c in child.children(&mut inner) {
                if c.kind() == "scoped_identifier" || c.kind() == "identifier" {
                    return Some(node_text(c, source).to_string());
                }
            }
        }
    }
    None
}

fn parse_type_declaration(
    node: tree_sitter::Node,
    source: &str,
    package_name: &str,
    pkg_id: i64,
    store: &Store,
) -> Result<()> {
    let name = node
        .child_by_field_name("name")
        .map(|n| node_text(n, source).to_string())
        .unwrap_or_default();

    if name.is_empty() {
        return Ok(());
    }

    let fqn = if package_name.is_empty() {
        name.clone()
    } else {
        format!("{}.{}", package_name, name)
    };

    let kind = match node.kind() {
        "class_declaration" => "class",
        "interface_declaration" => "interface",
        "enum_declaration" => "enum",
        "annotation_type_declaration" => "annotation",
        "record_declaration" => "record",
        _ => "class",
    };

    // Extract doc comment (preceding comment node)
    let doc_comment = extract_preceding_doc_comment(node, source);

    // Extract annotations
    let annotations = extract_annotations(node, source);

    // Extract superclass
    let superclass = node
        .child_by_field_name("superclass")
        .and_then(|sc| {
            let mut c = sc.walk();
            sc.children(&mut c)
                .find(|n| n.kind() == "type_identifier" || n.kind() == "scoped_type_identifier")
                .map(|n| node_text(n, source).to_string())
        });

    // Extract interfaces
    let interfaces_node = node.child_by_field_name("interfaces");
    let interfaces = interfaces_node.map(|ifaces| {
        let mut c = ifaces.walk();
        let list: Vec<String> = ifaces
            .children(&mut c)
            .filter(|n| n.kind() == "type_identifier" || n.kind() == "scoped_type_identifier")
            .map(|n| node_text(n, source).to_string())
            .collect();
        serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string())
    });

    let type_row = TypeRow {
        id: 0,
        package_id: pkg_id,
        name: name.clone(),
        fqn: fqn.clone(),
        kind: kind.to_string(),
        doc_comment,
        annotations: annotations.map(|a| serde_json::to_string(&a).unwrap_or_default()),
        superclass,
        interfaces,
    };
    let type_id = store.insert_type(&type_row)?;

    // Parse body for methods and fields
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            match child.kind() {
                "method_declaration" | "constructor_declaration" => {
                    parse_method(child, source, type_id, store)?;
                }
                "field_declaration" => {
                    parse_field(child, source, type_id, store)?;
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn parse_method(
    node: tree_sitter::Node,
    source: &str,
    type_id: i64,
    store: &Store,
) -> Result<()> {
    let name = node
        .child_by_field_name("name")
        .map(|n| node_text(n, source).to_string())
        .unwrap_or_else(|| "<init>".to_string());

    let signature = node_text(node, source)
        .lines()
        .next()
        .unwrap_or("")
        .trim_end_matches('{')
        .trim()
        .to_string();

    let return_type = node
        .child_by_field_name("type")
        .map(|n| node_text(n, source).to_string());

    let params = extract_params(node, source);
    let doc_comment = extract_preceding_doc_comment(node, source);
    let annotations = extract_annotations(node, source);
    let is_static = has_modifier(node, source, "static");

    let method_row = MethodRow {
        id: 0,
        type_id,
        name,
        signature,
        return_type,
        params: serde_json::to_string(&params).unwrap_or_else(|_| "[]".to_string()),
        doc_comment,
        annotations: annotations.map(|a| serde_json::to_string(&a).unwrap_or_default()),
        is_static,
    };
    store.insert_method(&method_row)?;
    Ok(())
}

fn parse_field(
    node: tree_sitter::Node,
    source: &str,
    type_id: i64,
    store: &Store,
) -> Result<()> {
    let field_type = node
        .child_by_field_name("type")
        .map(|n| node_text(n, source).to_string())
        .unwrap_or_default();

    // A field_declaration can have multiple declarators
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            let name = child
                .child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }

            let doc_comment = extract_preceding_doc_comment(node, source);
            let annotations = extract_annotations(node, source);
            let is_static = has_modifier(node, source, "static");

            let field_row = FieldRow {
                id: 0,
                type_id,
                name,
                field_type: field_type.clone(),
                doc_comment,
                annotations: annotations.map(|a| serde_json::to_string(&a).unwrap_or_default()),
                is_static,
            };
            store.insert_field(&field_row)?;
        }
    }
    Ok(())
}

fn extract_preceding_doc_comment(node: tree_sitter::Node, source: &str) -> Option<String> {
    let mut prev = node.prev_sibling();
    // Skip annotations to find doc comment
    while let Some(p) = prev {
        if p.kind() == "block_comment" || p.kind() == "line_comment" {
            let text = node_text(p, source);
            if text.starts_with("/**") {
                // Strip /** and */ and leading * from each line
                let cleaned = text
                    .trim_start_matches("/**")
                    .trim_end_matches("*/")
                    .lines()
                    .map(|l| l.trim().trim_start_matches('*').trim())
                    .filter(|l| !l.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                return Some(cleaned);
            }
            return None; // Non-javadoc comment, stop
        } else if p.kind() == "modifiers" || p.kind() == "marker_annotation" || p.kind() == "annotation" {
            prev = p.prev_sibling();
        } else {
            break;
        }
    }
    None
}

fn extract_annotations(node: tree_sitter::Node, source: &str) -> Option<Vec<String>> {
    let modifiers = node.child_by_field_name("modifiers")?;
    let mut annotations = Vec::new();
    let mut cursor = modifiers.walk();
    for child in modifiers.children(&mut cursor) {
        if child.kind() == "marker_annotation"
            || child.kind() == "annotation"
            || child.kind() == "single_element_annotation"
        {
            annotations.push(node_text(child, source).to_string());
        }
    }
    if annotations.is_empty() {
        None
    } else {
        Some(annotations)
    }
}

fn has_modifier(node: tree_sitter::Node, source: &str, modifier: &str) -> bool {
    if let Some(modifiers) = node.child_by_field_name("modifiers") {
        let mut cursor = modifiers.walk();
        for child in modifiers.children(&mut cursor) {
            if node_text(child, source) == modifier {
                return true;
            }
        }
    }
    false
}

#[derive(serde::Serialize)]
struct ParamInfo {
    name: String,
    #[serde(rename = "type")]
    param_type: String,
}

fn extract_params(method_node: tree_sitter::Node, source: &str) -> Vec<ParamInfo> {
    let mut params = Vec::new();
    if let Some(param_list) = method_node.child_by_field_name("parameters") {
        let mut cursor = param_list.walk();
        for child in param_list.children(&mut cursor) {
            if child.kind() == "formal_parameter" || child.kind() == "spread_parameter" {
                let param_type = child
                    .child_by_field_name("type")
                    .map(|n| node_text(n, source).to_string())
                    .unwrap_or_default();
                let name = child
                    .child_by_field_name("name")
                    .map(|n| node_text(n, source).to_string())
                    .unwrap_or_default();
                params.push(ParamInfo { name, param_type });
            }
        }
    }
    params
}

fn node_text<'a>(node: tree_sitter::Node, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use tempfile::NamedTempFile;

    fn test_store() -> Store {
        let tmp = NamedTempFile::new().unwrap();
        Store::open(tmp.path()).unwrap()
    }

    #[test]
    fn test_parse_simple_class() {
        let store = test_store();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&tree_sitter_java::LANGUAGE.into()).unwrap();

        let source = r#"
package com.example;

/**
 * A sample class.
 */
public class Foo extends Bar implements Baz {
    private String name;

    /**
     * Does something.
     */
    public void doSomething(String input) {
        // body
    }

    public static int count() {
        return 0;
    }
}
"#;
        parse_java_file(&mut parser, source, &store).unwrap();

        let packages = store.list_packages().unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "com.example");

        let types = store.list_types_in_package("com.example").unwrap();
        assert_eq!(types.len(), 1);
        assert_eq!(types[0].name, "Foo");
        assert_eq!(types[0].kind, "class");
        assert!(types[0].doc_comment.as_ref().unwrap().contains("sample class"));

        let methods = store.get_methods_for_type(types[0].id).unwrap();
        assert_eq!(methods.len(), 2);

        let fields = store.get_fields_for_type(types[0].id).unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "name");
    }

    #[test]
    fn test_parse_interface() {
        let store = test_store();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&tree_sitter_java::LANGUAGE.into()).unwrap();

        let source = r#"
package com.example;

public interface MyInterface {
    void doIt();
    String getName();
}
"#;
        parse_java_file(&mut parser, source, &store).unwrap();
        let types = store.list_types_in_package("com.example").unwrap();
        assert_eq!(types[0].kind, "interface");
        let methods = store.get_methods_for_type(types[0].id).unwrap();
        assert_eq!(methods.len(), 2);
    }

    #[test]
    fn test_parse_annotated_class() {
        let store = test_store();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&tree_sitter_java::LANGUAGE.into()).unwrap();

        let source = r#"
package com.example;

@Deprecated
public class Old {
    @Override
    public String toString() { return "old"; }
}
"#;
        parse_java_file(&mut parser, source, &store).unwrap();
        let types = store.list_types_in_package("com.example").unwrap();
        assert!(types[0].annotations.as_ref().unwrap().contains("@Deprecated"));
    }

    #[test]
    fn test_search_after_parsing() {
        let store = test_store();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&tree_sitter_java::LANGUAGE.into()).unwrap();

        let source = r#"
package com.example;

/**
 * Immutable list implementation.
 */
public class ImmutableList {
    /**
     * Creates an empty immutable list.
     */
    public static ImmutableList of() { return null; }
}
"#;
        parse_java_file(&mut parser, source, &store).unwrap();
        let results = store.search("immutable list", 10).unwrap();
        assert!(!results.is_empty());
    }
}
```

**Step 3: Run tests**

Run: `cargo test parser::source`
Expected: All 4 tests pass

**Step 4: Commit**

```bash
git add -A && git commit -m "feat: source JAR parser using tree-sitter-java"
```

---

### Task 7: Javadoc HTML Parser

**Files:**
- Create: `src/parser/javadoc.rs`

**Step 1: Implement javadoc HTML parser**

```rust
// src/parser/javadoc.rs
use crate::store::{FieldRow, MethodRow, Store, TypeRow};
use anyhow::Result;
use scraper::{Html, Selector};
use std::path::Path;
use tracing::warn;

pub fn parse_javadoc_dir(dir: &Path, store: &Store) -> Result<()> {
    walk_html_files(dir, &mut |path| {
        // Skip non-class pages (package-summary, index, etc.)
        let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
        if filename.starts_with("package-")
            || filename.starts_with("index")
            || filename.starts_with("allclasses")
            || filename.starts_with("allpackages")
            || filename.starts_with("overview")
            || filename == "help-doc.html"
            || filename == "deprecated-list.html"
            || filename == "serialized-form.html"
            || filename == "constant-values.html"
        {
            return Ok(());
        }

        let content = std::fs::read_to_string(path)?;
        if let Err(e) = parse_class_page(&content, dir, path, store) {
            warn!("Failed to parse javadoc page {}: {}", path.display(), e);
        }
        Ok(())
    })?;

    // Also parse package-summary pages for package docs
    walk_html_files(dir, &mut |path| {
        let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
        if filename == "package-summary.html" {
            let content = std::fs::read_to_string(path)?;
            if let Err(e) = parse_package_summary(&content, dir, path, store) {
                warn!("Failed to parse package summary {}: {}", path.display(), e);
            }
        }
        Ok(())
    })?;

    Ok(())
}

fn walk_html_files(dir: &Path, handler: &mut dyn FnMut(&Path) -> Result<()>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_html_files(&path, handler)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("html") {
            handler(&path)?;
        }
    }
    Ok(())
}

fn path_to_package(base_dir: &Path, file_path: &Path) -> String {
    let relative = file_path.parent().unwrap_or(file_path);
    let relative = relative.strip_prefix(base_dir).unwrap_or(relative);
    relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(".")
}

fn parse_package_summary(html: &str, base_dir: &Path, path: &Path, store: &Store) -> Result<()> {
    let doc = Html::parse_document(html);
    let package_name = path_to_package(base_dir, path);
    if package_name.is_empty() {
        return Ok(());
    }

    // Try to extract package description
    let desc_sel = Selector::parse(".package-description .block, .contentContainer .block")
        .unwrap();
    let doc_comment = doc.select(&desc_sel).next().map(|el| el.text().collect::<String>());

    store.insert_package(&package_name, doc_comment.as_deref())?;
    Ok(())
}

fn parse_class_page(html: &str, base_dir: &Path, path: &Path, store: &Store) -> Result<()> {
    let doc = Html::parse_document(html);
    let package_name = path_to_package(base_dir, path);

    // Extract class name from title or heading
    let title_sel = Selector::parse("h1.title, h2.title, .header h1, .header h2").unwrap();
    let class_name_raw = doc
        .select(&title_sel)
        .next()
        .map(|el| el.text().collect::<String>())
        .unwrap_or_default();

    // Parse "Class Foo<T>" or "Interface Bar" etc.
    let (kind, class_name) = parse_type_heading(&class_name_raw);
    if class_name.is_empty() {
        // Fallback: derive from filename
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if stem.is_empty() {
            return Ok(());
        }
        return parse_class_page_with_name(
            &doc, &package_name, stem, "class", store,
        );
    }

    parse_class_page_with_name(&doc, &package_name, &class_name, kind, store)
}

fn parse_type_heading(heading: &str) -> (&str, String) {
    let heading = heading.trim();
    let prefixes = [
        ("Class ", "class"),
        ("Interface ", "interface"),
        ("Enum ", "enum"),
        ("Enum Class ", "enum"),
        ("Annotation Type ", "annotation"),
        ("Annotation Interface ", "annotation"),
        ("Record Class ", "record"),
    ];
    for (prefix, kind) in prefixes {
        if let Some(rest) = heading.strip_prefix(prefix) {
            // Strip type parameters
            let name = rest.split('<').next().unwrap_or(rest).trim().to_string();
            return (kind, name);
        }
    }
    // Unknown prefix, try to use whole thing
    let name = heading.split('<').next().unwrap_or(heading).trim().to_string();
    ("class", name)
}

fn parse_class_page_with_name(
    doc: &Html,
    package_name: &str,
    class_name: &str,
    kind: &str,
    store: &Store,
) -> Result<()> {
    let pkg_id = store.insert_package(package_name, None)?;
    let fqn = if package_name.is_empty() {
        class_name.to_string()
    } else {
        format!("{}.{}", package_name, class_name)
    };

    // Extract class-level doc
    let desc_sel = Selector::parse(".class-description .block, .description .block, .contentContainer > .description .block").unwrap();
    let doc_comment = doc.select(&desc_sel).next().map(|el| el.text().collect::<String>());

    // Extract superclass from "extends" info
    let extends_sel = Selector::parse(".extends-implements, .description .inheritance, dt:contains('extends') + dd").unwrap();
    let superclass = doc.select(&extends_sel).next().map(|el| el.text().collect::<String>());

    let type_row = TypeRow {
        id: 0,
        package_id: pkg_id,
        name: class_name.to_string(),
        fqn,
        kind: kind.to_string(),
        doc_comment,
        annotations: None,
        superclass,
        interfaces: None,
    };
    let type_id = store.insert_type(&type_row)?;

    // Parse method summaries
    parse_method_details(doc, type_id, store)?;

    // Parse field summaries
    parse_field_details(doc, type_id, store)?;

    Ok(())
}

fn parse_method_details(doc: &Html, type_id: i64, store: &Store) -> Result<()> {
    // Modern javadoc (11+) uses <section class="method-details">
    // Older uses <a name="method.detail"> or <h3>Method Detail</h3>
    let detail_sel = Selector::parse(
        ".method-details .member-list > li, \
         .method-detail .member-list > li, \
         #method-detail ul.member-list > li, \
         section.method-details ul.member-list > li"
    ).unwrap();

    // Fallback for older javadoc
    let old_detail_sel = Selector::parse(
        ".details .blockList .blockList, \
         .memberSummary tbody tr"
    ).unwrap();

    let sig_sel = Selector::parse(".member-signature, .memberSignature, pre").unwrap();
    let block_sel = Selector::parse(".block").unwrap();

    let selectors_to_try = [&detail_sel, &old_detail_sel];

    for sel in selectors_to_try {
        let elements: Vec<_> = doc.select(sel).collect();
        if elements.is_empty() {
            continue;
        }

        for el in elements {
            let sig = el.select(&sig_sel).next().map(|s| s.text().collect::<String>());
            let description = el.select(&block_sel).next().map(|b| b.text().collect::<String>());

            let signature = sig.unwrap_or_default().trim().to_string();
            if signature.is_empty() {
                continue;
            }

            let name = extract_method_name_from_sig(&signature);

            let method_row = MethodRow {
                id: 0,
                type_id,
                name,
                signature: signature.clone(),
                return_type: None, // Embedded in signature
                params: "[]".to_string(),
                doc_comment: description,
                annotations: None,
                is_static: signature.contains("static "),
            };
            store.insert_method(&method_row)?;
        }
        break; // Use first selector that matched
    }

    Ok(())
}

fn parse_field_details(doc: &Html, type_id: i64, store: &Store) -> Result<()> {
    let detail_sel = Selector::parse(
        ".field-details .member-list > li, \
         section.field-details ul.member-list > li"
    ).unwrap();

    let sig_sel = Selector::parse(".member-signature, .memberSignature, pre").unwrap();
    let block_sel = Selector::parse(".block").unwrap();

    for el in doc.select(&detail_sel) {
        let sig = el.select(&sig_sel).next().map(|s| s.text().collect::<String>());
        let description = el.select(&block_sel).next().map(|b| b.text().collect::<String>());

        let signature = sig.unwrap_or_default().trim().to_string();
        if signature.is_empty() {
            continue;
        }

        // Parse "public static final String NAME" style
        let parts: Vec<&str> = signature.split_whitespace().collect();
        let name = parts.last().unwrap_or(&"").to_string();
        let field_type = parts.iter().rev().nth(1).unwrap_or(&"").to_string();

        let field_row = FieldRow {
            id: 0,
            type_id,
            name,
            field_type,
            doc_comment: description,
            annotations: None,
            is_static: signature.contains("static "),
        };
        store.insert_field(&field_row)?;
    }

    Ok(())
}

fn extract_method_name_from_sig(sig: &str) -> String {
    // Find the method name: it's the identifier before the first '('
    if let Some(paren_idx) = sig.find('(') {
        let before_paren = sig[..paren_idx].trim();
        // Last word before '(' is the method name
        before_paren
            .split_whitespace()
            .last()
            .unwrap_or("")
            .to_string()
    } else {
        sig.split_whitespace().last().unwrap_or("").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_type_heading() {
        assert_eq!(parse_type_heading("Class ImmutableList<E>"), ("class", "ImmutableList".into()));
        assert_eq!(parse_type_heading("Interface Predicate<T>"), ("interface", "Predicate".into()));
        assert_eq!(parse_type_heading("Enum Color"), ("enum", "Color".into()));
        assert_eq!(parse_type_heading("Annotation Type Override"), ("annotation", "Override".into()));
        assert_eq!(parse_type_heading("Record Class Point"), ("record", "Point".into()));
    }

    #[test]
    fn test_extract_method_name_from_sig() {
        assert_eq!(extract_method_name_from_sig("public void doSomething(String s)"), "doSomething");
        assert_eq!(extract_method_name_from_sig("static <T> List<T> of(T... elements)"), "of");
        assert_eq!(extract_method_name_from_sig("String getName()"), "getName");
    }

    #[test]
    fn test_path_to_package() {
        let base = Path::new("/tmp/docs");
        let file = Path::new("/tmp/docs/com/example/Foo.html");
        assert_eq!(path_to_package(base, file), "com.example");
    }
}
```

**Step 2: Run tests**

Run: `cargo test parser::javadoc`
Expected: All 3 tests pass

**Step 3: Commit**

```bash
git add -A && git commit -m "feat: javadoc HTML parser for extracting docs from javadoc JARs"
```

---

### Task 8: MCP Tools

**Files:**
- Create: `src/tools.rs`

**Step 1: Implement the 3 MCP tools**

```rust
// src/tools.rs
use crate::config::AppConfig;
use crate::parser;
use crate::resolver;
use crate::store::{LibraryId, Store};
use anyhow::Result;
use rmcp::model::{CallToolResult, Content};
use rmcp::tool_router::Parameters;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResolveLibraryParams {
    /// Library query - e.g. "guava 33.0" or "com.google.guava:guava:33.0.0-jre"
    #[schemars(description = "Library identifier. Use 'groupId:artifactId:version' format, or a natural name like 'guava 33.0'")]
    pub query: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct QueryDocsParams {
    /// Library ID from resolve_library (format: groupId:artifactId:version)
    #[schemars(description = "Library ID returned by resolve_library, in groupId:artifactId:version format")]
    pub library_id: String,

    /// Free-text search query
    #[schemars(description = "Search query to find relevant documentation")]
    pub query: String,

    /// Maximum results (default 20)
    #[schemars(description = "Maximum number of results to return")]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BrowseLibraryParams {
    /// Library ID from resolve_library
    #[schemars(description = "Library ID returned by resolve_library, in groupId:artifactId:version format")]
    pub library_id: String,

    /// Navigation path: empty for packages, package name for classes, FQN for class details
    #[schemars(description = "Navigation path. Empty or omitted: list packages. Package name (e.g. 'com.google.common.collect'): list classes. Fully qualified class name: show full class docs with all members.")]
    pub path: Option<String>,
}

pub struct OssContextServer {
    config: Arc<AppConfig>,
}

impl OssContextServer {
    pub fn new(config: AppConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }

    fn parse_library_id(query: &str) -> Option<LibraryId> {
        let parts: Vec<&str> = query.split(':').collect();
        if parts.len() == 3 {
            Some(LibraryId {
                group_id: parts[0].to_string(),
                artifact_id: parts[1].to_string(),
                version: parts[2].to_string(),
            })
        } else {
            None
        }
    }

    fn open_store(&self, lib: &LibraryId) -> Result<Option<Store>> {
        let db_path = lib.db_path(&self.config.cache_dir);
        Store::open_if_exists(&db_path)
    }

    fn resolve_and_index(&self, lib: &LibraryId) -> Result<Store> {
        let db_path = lib.db_path(&self.config.cache_dir);
        let store = Store::open(&db_path)?;

        let jar = resolver::resolve(lib, &self.config)?;
        parser::index_jar(&jar, lib, &store)?;

        Ok(store)
    }
}

#[rmcp::tool_router]
impl OssContextServer {
    #[tool(description = "Resolve a Java library and make its documentation available for querying. Accepts 'groupId:artifactId:version' format. Returns the library ID to use with query_docs and browse_library.")]
    async fn resolve_library(
        &self,
        Parameters(params): Parameters<ResolveLibraryParams>,
    ) -> CallToolResult {
        let query = params.query.trim();
        info!("Resolving library: {}", query);

        let lib = match Self::parse_library_id(query) {
            Some(lib) => lib,
            None => {
                return Ok(CallToolResult {
                    content: vec![Content::text(format!(
                        "Could not parse library identifier '{}'. Please use groupId:artifactId:version format (e.g. 'com.google.guava:guava:33.0.0-jre')",
                        query
                    ))],
                    is_error: Some(true),
                    ..Default::default()
                });
            }
        };

        // Check if already indexed
        match self.open_store(&lib) {
            Ok(Some(store)) if store.has_library_meta().unwrap_or(false) => {
                return Ok(CallToolResult {
                    content: vec![Content::text(format!(
                        "Library {} is ready. Use this ID with query_docs and browse_library: {}",
                        lib.to_string_id(),
                        lib.to_string_id()
                    ))],
                    is_error: None,
                    ..Default::default()
                });
            }
            _ => {}
        }

        // Resolve and index
        match self.resolve_and_index(&lib) {
            Ok(_) => Ok(CallToolResult {
                content: vec![Content::text(format!(
                    "Library {} indexed successfully. Use this ID with query_docs and browse_library: {}",
                    lib.to_string_id(),
                    lib.to_string_id()
                ))],
                is_error: None,
                ..Default::default()
            }),
            Err(e) => {
                error!("Failed to resolve {}: {}", lib.to_string_id(), e);
                Ok(CallToolResult {
                    content: vec![Content::text(format!(
                        "Could not find documentation for {}: {}",
                        lib.to_string_id(),
                        e
                    ))],
                    is_error: Some(true),
                    ..Default::default()
                })
            }
        }
    }

    #[tool(description = "Search documentation across a resolved Java library using free-text queries. Returns matching classes, methods, and fields ranked by relevance.")]
    async fn query_docs(
        &self,
        Parameters(params): Parameters<QueryDocsParams>,
    ) -> CallToolResult {
        let lib = match Self::parse_library_id(&params.library_id) {
            Some(lib) => lib,
            None => {
                return Ok(CallToolResult {
                    content: vec![Content::text("Invalid library_id format. Use groupId:artifactId:version".to_string())],
                    is_error: Some(true),
                    ..Default::default()
                });
            }
        };

        let store = match self.open_store(&lib) {
            Ok(Some(store)) => store,
            _ => {
                return Ok(CallToolResult {
                    content: vec![Content::text(format!(
                        "Library {} is not indexed. Call resolve_library first.",
                        params.library_id
                    ))],
                    is_error: Some(true),
                    ..Default::default()
                });
            }
        };

        let limit = params.limit.unwrap_or(20);
        match store.search(&params.query, limit) {
            Ok(results) if results.is_empty() => Ok(CallToolResult {
                content: vec![Content::text(format!(
                    "No results found for '{}' in {}",
                    params.query, params.library_id
                ))],
                is_error: None,
                ..Default::default()
            }),
            Ok(results) => {
                let mut output = format!("Found {} results for '{}' in {}:\n\n", results.len(), params.query, params.library_id);
                for (i, r) in results.iter().enumerate() {
                    output.push_str(&format!("{}. [{}] {}\n", i + 1, r.kind, r.fqn));
                    if let Some(ref sig) = r.signature {
                        if !sig.is_empty() {
                            output.push_str(&format!("   Signature: {}\n", sig));
                        }
                    }
                    if let Some(ref doc) = r.doc_comment {
                        let truncated: String = doc.chars().take(200).collect();
                        output.push_str(&format!("   {}\n", truncated));
                    }
                    output.push('\n');
                }
                Ok(CallToolResult {
                    content: vec![Content::text(output)],
                    is_error: None,
                    ..Default::default()
                })
            }
            Err(e) => Ok(CallToolResult {
                content: vec![Content::text(format!("Search error: {}", e))],
                is_error: Some(true),
                ..Default::default()
            }),
        }
    }

    #[tool(description = "Browse a Java library's documentation hierarchically. Without a path: list packages. With a package name: list classes. With a fully qualified class name: show full documentation including all methods and fields.")]
    async fn browse_library(
        &self,
        Parameters(params): Parameters<BrowseLibraryParams>,
    ) -> CallToolResult {
        let lib = match Self::parse_library_id(&params.library_id) {
            Some(lib) => lib,
            None => {
                return Ok(CallToolResult {
                    content: vec![Content::text("Invalid library_id format.".to_string())],
                    is_error: Some(true),
                    ..Default::default()
                });
            }
        };

        let store = match self.open_store(&lib) {
            Ok(Some(store)) => store,
            _ => {
                return Ok(CallToolResult {
                    content: vec![Content::text(format!(
                        "Library {} is not indexed. Call resolve_library first.",
                        params.library_id
                    ))],
                    is_error: Some(true),
                    ..Default::default()
                });
            }
        };

        let path = params.path.as_deref().unwrap_or("");

        if path.is_empty() {
            // List packages
            match store.list_packages() {
                Ok(packages) => {
                    let mut output = format!("Packages in {}:\n\n", params.library_id);
                    for p in &packages {
                        output.push_str(&format!("  {}\n", p.name));
                        if let Some(ref doc) = p.doc_comment {
                            let short: String = doc.chars().take(100).collect();
                            output.push_str(&format!("    {}\n", short));
                        }
                    }
                    Ok(CallToolResult {
                        content: vec![Content::text(output)],
                        is_error: None,
                        ..Default::default()
                    })
                }
                Err(e) => Ok(CallToolResult {
                    content: vec![Content::text(format!("Error: {}", e))],
                    is_error: Some(true),
                    ..Default::default()
                }),
            }
        } else if path.chars().last().map_or(true, |c| c.is_lowercase() || c == '.') {
            // Looks like a package name (lowercase) — list types in package
            match store.list_types_in_package(path) {
                Ok(types) if types.is_empty() => {
                    // Maybe it's actually a FQN - try as type
                    browse_type_detail(&store, path, &params.library_id)
                }
                Ok(types) => {
                    let mut output = format!("Types in {}:\n\n", path);
                    for t in &types {
                        output.push_str(&format!("  {} {} {}\n", t.kind, t.fqn,
                            t.superclass.as_deref().map(|s| format!("extends {}", s)).unwrap_or_default()));
                        if let Some(ref doc) = t.doc_comment {
                            let short: String = doc.chars().take(100).collect();
                            output.push_str(&format!("    {}\n", short));
                        }
                    }
                    Ok(CallToolResult {
                        content: vec![Content::text(output)],
                        is_error: None,
                        ..Default::default()
                    })
                }
                Err(e) => Ok(CallToolResult {
                    content: vec![Content::text(format!("Error: {}", e))],
                    is_error: Some(true),
                    ..Default::default()
                }),
            }
        } else {
            // Looks like a FQN (has uppercase) — show class detail
            browse_type_detail(&store, path, &params.library_id)
        }
    }
}

fn browse_type_detail(store: &Store, fqn: &str, library_id: &str) -> CallToolResult {
    match store.get_type_by_fqn(fqn) {
        Ok(Some(t)) => {
            let mut output = format!("{} {}\n", t.kind, t.fqn);
            if let Some(ref sc) = t.superclass {
                output.push_str(&format!("  extends {}\n", sc));
            }
            if let Some(ref ifaces) = t.interfaces {
                output.push_str(&format!("  implements {}\n", ifaces));
            }
            if let Some(ref ann) = t.annotations {
                output.push_str(&format!("  annotations: {}\n", ann));
            }
            if let Some(ref doc) = t.doc_comment {
                output.push_str(&format!("\n{}\n", doc));
            }

            // Fields
            if let Ok(fields) = store.get_fields_for_type(t.id) {
                if !fields.is_empty() {
                    output.push_str("\n--- Fields ---\n\n");
                    for f in &fields {
                        output.push_str(&format!("  {} {}\n", f.field_type, f.name));
                        if let Some(ref doc) = f.doc_comment {
                            output.push_str(&format!("    {}\n", doc));
                        }
                    }
                }
            }

            // Methods
            if let Ok(methods) = store.get_methods_for_type(t.id) {
                if !methods.is_empty() {
                    output.push_str("\n--- Methods ---\n\n");
                    for m in &methods {
                        output.push_str(&format!("  {}\n", m.signature));
                        if let Some(ref doc) = m.doc_comment {
                            output.push_str(&format!("    {}\n", doc));
                        }
                        output.push('\n');
                    }
                }
            }

            Ok(CallToolResult {
                content: vec![Content::text(output)],
                is_error: None,
                ..Default::default()
            })
        }
        Ok(None) => Ok(CallToolResult {
            content: vec![Content::text(format!(
                "Type '{}' not found in {}. Try browse_library with the package name to see available types.",
                fqn, library_id
            ))],
            is_error: Some(true),
            ..Default::default()
        }),
        Err(e) => Ok(CallToolResult {
            content: vec![Content::text(format!("Error: {}", e))],
            is_error: Some(true),
            ..Default::default()
        }),
    }
}

// ServerHandler implementation
#[rmcp::tool_handler]
impl rmcp::ServerHandler for OssContextServer {
    fn get_info(&self) -> rmcp::model::InitializeResult {
        rmcp::model::InitializeResult {
            protocol_version: "2024-11-05".to_string(),
            capabilities: rmcp::model::ServerCapabilities {
                tools: Some(Default::default()),
                ..Default::default()
            },
            server_info: rmcp::model::Implementation {
                name: "oss-context".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            ..Default::default()
        }
    }
}
```

**Step 2: Run compilation check**

Run: `cargo check`
Expected: Compiles (may have warnings)

**Step 3: Commit**

```bash
git add -A && git commit -m "feat: MCP tool handlers for resolve_library, query_docs, browse_library"
```

---

### Task 9: Transport & Main Wiring

**Files:**
- Modify: `src/transport.rs`
- Modify: `src/main.rs`

**Step 1: Implement transport module**

```rust
// src/transport.rs
use crate::tools::OssContextServer;
use anyhow::Result;
use rmcp::ServiceExt;

pub async fn serve_stdio(server: OssContextServer) -> Result<()> {
    tracing::info!("Starting MCP server on stdio");
    let service = server.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}

pub async fn serve_sse(server: OssContextServer, port: u16) -> Result<()> {
    tracing::info!("Starting MCP server on SSE port {}", port);
    // SSE transport setup depends on rmcp version — this is a placeholder
    // that will need adjustment based on actual rmcp SSE API
    todo!("SSE transport - implement based on rmcp SSE API")
}
```

**Step 2: Wire up main.rs**

```rust
// src/main.rs
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
        transport: cli.transport.clone(),
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
    tracing::info!(?config, "Configuration loaded");

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
```

**Step 3: Verify compilation**

Run: `cargo build`
Expected: Compiles successfully

**Step 4: Commit**

```bash
git add -A && git commit -m "feat: wire up transport and main entry point"
```

---

### Task 10: Integration Test

**Files:**
- Create: `tests/integration.rs`

**Step 1: Write an integration test that parses source and queries**

```rust
// tests/integration.rs
use oss_context::store::{LibraryId, Store};
use oss_context::parser::source::parse_source_dir;
use tempfile::{NamedTempFile, TempDir};
use std::fs;

#[test]
fn test_end_to_end_source_indexing_and_search() {
    // Create a fake source JAR directory
    let src_dir = TempDir::new().unwrap();
    let pkg_dir = src_dir.path().join("com").join("example");
    fs::create_dir_all(&pkg_dir).unwrap();

    fs::write(pkg_dir.join("Calculator.java"), r#"
package com.example;

/**
 * A simple calculator for arithmetic operations.
 */
public class Calculator {
    /**
     * Adds two numbers together.
     * @param a first number
     * @param b second number
     * @return the sum
     */
    public int add(int a, int b) {
        return a + b;
    }

    /**
     * Subtracts b from a.
     */
    public int subtract(int a, int b) {
        return a - b;
    }

    /**
     * Multiplies two numbers.
     */
    public static int multiply(int a, int b) {
        return a * b;
    }
}
"#).unwrap();

    fs::write(pkg_dir.join("StringUtils.java"), r#"
package com.example;

/**
 * Utility class for string operations.
 */
public final class StringUtils {
    /**
     * Checks if a string is empty or null.
     */
    public static boolean isEmpty(String s) {
        return s == null || s.isEmpty();
    }
}
"#).unwrap();

    // Create store and index
    let db_file = NamedTempFile::new().unwrap();
    let store = Store::open(db_file.path()).unwrap();
    parse_source_dir(src_dir.path(), &store).unwrap();

    // Test package listing
    let packages = store.list_packages().unwrap();
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].name, "com.example");

    // Test type listing
    let types = store.list_types_in_package("com.example").unwrap();
    assert_eq!(types.len(), 2);

    // Test search
    let results = store.search("calculator arithmetic", 10).unwrap();
    assert!(!results.is_empty());
    assert!(results[0].fqn.contains("Calculator"));

    // Test search for method
    let results = store.search("adds two numbers", 10).unwrap();
    assert!(!results.is_empty());

    // Test browse by FQN
    let calc = store.get_type_by_fqn("com.example.Calculator").unwrap().unwrap();
    assert_eq!(calc.kind, "class");

    let methods = store.get_methods_for_type(calc.id).unwrap();
    assert_eq!(methods.len(), 3); // add, subtract, multiply

    // Verify static method
    let multiply = methods.iter().find(|m| m.name == "multiply").unwrap();
    assert!(multiply.is_static);
}
```

**Step 2: Run the integration test**

Run: `cargo test --test integration`
Expected: PASS

**Step 3: Commit**

```bash
git add -A && git commit -m "test: add end-to-end integration test for source indexing and search"
```

---

### Task 11: Final Cleanup

**Step 1:** Run `cargo clippy` and fix any warnings
**Step 2:** Run `cargo test` for all tests
**Step 3:** Run `cargo build --release` to verify release build
**Step 4:** Commit any fixes

```bash
git add -A && git commit -m "chore: fix clippy warnings and verify release build"
```
