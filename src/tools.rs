use crate::config::AppConfig;
use crate::mcp::{CallToolResult, Content, ToolDef};
use crate::parser;
use crate::resolver;
use crate::store::{LibraryId, Store};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use tracing::{error, info};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResolveLibraryParams {
    /// Library identifier. Use 'groupId:artifactId:version' format (e.g. 'com.google.guava:guava:33.0.0-jre')
    pub query: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct QueryDocsParams {
    /// Library ID returned by resolve_library, in groupId:artifactId:version format
    pub library_id: String,
    /// Search query to find relevant documentation
    pub query: String,
    /// Maximum number of results to return (default 20)
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BrowseLibraryParams {
    /// Library ID returned by resolve_library, in groupId:artifactId:version format
    pub library_id: String,
    /// Navigation path. Empty or omitted: list packages. Package name: list classes. FQN: show full class docs.
    pub path: Option<String>,
}

#[derive(Clone)]
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

    fn open_store(&self, lib: &LibraryId) -> Option<Store> {
        let db_path = lib.db_path(&self.config.cache_dir);
        Store::open_if_exists(&db_path).ok().flatten()
    }

    fn resolve_and_index(&self, lib: &LibraryId) -> Result<Store, String> {
        let db_path = lib.db_path(&self.config.cache_dir);
        let store = Store::open(&db_path).map_err(|e| format!("Failed to open store: {}", e))?;
        let jar = resolver::resolve(lib, &self.config).map_err(|e| e.to_string())?;
        parser::index_jar(&jar, lib, &store).map_err(|e| format!("Failed to index: {}", e))?;
        Ok(store)
    }

    pub fn tool_definitions() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "resolve_library".to_string(),
                description: "Resolve a Java library and make its documentation available for querying. Accepts 'groupId:artifactId:version' format. Returns the library ID to use with query_docs and browse_library.".to_string(),
                input_schema: schemars::schema_for!(ResolveLibraryParams).to_value(),
            },
            ToolDef {
                name: "query_docs".to_string(),
                description: "Search documentation across a resolved Java library using free-text queries. Returns matching classes, methods, and fields ranked by relevance.".to_string(),
                input_schema: schemars::schema_for!(QueryDocsParams).to_value(),
            },
            ToolDef {
                name: "browse_library".to_string(),
                description: "Browse a Java library's documentation hierarchically. Without a path: list packages. With a package name: list classes. With a fully qualified class name: show full documentation including all methods and fields.".to_string(),
                input_schema: schemars::schema_for!(BrowseLibraryParams).to_value(),
            },
        ]
    }

    pub async fn call_tool(&self, name: &str, args: Value) -> CallToolResult {
        match name {
            "resolve_library" => self.resolve_library(args).await,
            "query_docs" => self.query_docs(args).await,
            "browse_library" => self.browse_library(args).await,
            _ => CallToolResult::error(vec![Content::text(format!("Unknown tool: {}", name))]),
        }
    }

    async fn resolve_library(&self, args: Value) -> CallToolResult {
        let params: ResolveLibraryParams = match serde_json::from_value(args) {
            Ok(p) => p,
            Err(e) => return CallToolResult::error(vec![Content::text(format!("Invalid parameters: {}", e))]),
        };

        let query = params.query.trim().to_string();
        info!("Resolving library: {}", query);

        let lib = match Self::parse_library_id(&query) {
            Some(lib) => lib,
            None => {
                return CallToolResult::error(vec![Content::text(format!(
                    "Could not parse library identifier '{}'. Please use groupId:artifactId:version format (e.g. 'com.google.guava:guava:33.0.0-jre')",
                    query
                ))]);
            }
        };

        let server = self.clone();
        let lib_clone = lib.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            if let Some(store) = server.open_store(&lib_clone) {
                if store.has_library_meta().unwrap_or(false) {
                    return Ok(format!(
                        "Library {} is ready. Use this ID with query_docs and browse_library: {}",
                        lib_clone.to_string_id(),
                        lib_clone.to_string_id()
                    ));
                }
            }

            server.resolve_and_index(&lib_clone)?;
            Ok(format!(
                "Library {} indexed successfully. Use this ID with query_docs and browse_library: {}",
                lib_clone.to_string_id(),
                lib_clone.to_string_id()
            ))
        })
        .await;

        match result {
            Ok(Ok(msg)) => CallToolResult::success(vec![Content::text(msg)]),
            Ok(Err(e)) => {
                error!("Failed to resolve {}: {}", lib.to_string_id(), e);
                CallToolResult::error(vec![Content::text(format!(
                    "Could not find documentation for {}: {}",
                    lib.to_string_id(),
                    e
                ))])
            }
            Err(e) => CallToolResult::error(vec![Content::text(format!("Task join error: {}", e))]),
        }
    }

    async fn query_docs(&self, args: Value) -> CallToolResult {
        let params: QueryDocsParams = match serde_json::from_value(args) {
            Ok(p) => p,
            Err(e) => return CallToolResult::error(vec![Content::text(format!("Invalid parameters: {}", e))]),
        };

        let lib = match Self::parse_library_id(&params.library_id) {
            Some(lib) => lib,
            None => {
                return CallToolResult::error(vec![Content::text(
                    "Invalid library_id format. Use groupId:artifactId:version",
                )]);
            }
        };

        let server = self.clone();
        let query = params.query.clone();
        let library_id = params.library_id.clone();
        let limit = params.limit.unwrap_or(20);

        let result = tokio::task::spawn_blocking(move || {
            let store = match server.open_store(&lib) {
                Some(store) => store,
                None => {
                    return CallToolResult::error(vec![Content::text(format!(
                        "Library {} is not indexed. Call resolve_library first.",
                        library_id
                    ))]);
                }
            };

            match store.search(&query, limit) {
                Ok(results) if results.is_empty() => CallToolResult::success(vec![
                    Content::text(format!("No results found for '{}' in {}", query, library_id)),
                ]),
                Ok(results) => {
                    let mut output = format!(
                        "Found {} results for '{}' in {}:\n\n",
                        results.len(),
                        query,
                        library_id
                    );
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
                    CallToolResult::success(vec![Content::text(output)])
                }
                Err(e) => CallToolResult::error(vec![Content::text(format!(
                    "Search error: {}",
                    e
                ))]),
            }
        })
        .await;

        match result {
            Ok(r) => r,
            Err(e) => CallToolResult::error(vec![Content::text(format!("Task join error: {}", e))]),
        }
    }

    async fn browse_library(&self, args: Value) -> CallToolResult {
        let params: BrowseLibraryParams = match serde_json::from_value(args) {
            Ok(p) => p,
            Err(e) => return CallToolResult::error(vec![Content::text(format!("Invalid parameters: {}", e))]),
        };

        let lib = match Self::parse_library_id(&params.library_id) {
            Some(lib) => lib,
            None => {
                return CallToolResult::error(vec![Content::text(
                    "Invalid library_id format.",
                )]);
            }
        };

        let server = self.clone();
        let library_id = params.library_id.clone();
        let path = params.path.clone().unwrap_or_default();

        let result = tokio::task::spawn_blocking(move || {
            let store = match server.open_store(&lib) {
                Some(store) => store,
                None => {
                    return CallToolResult::error(vec![Content::text(format!(
                        "Library {} is not indexed. Call resolve_library first.",
                        library_id
                    ))]);
                }
            };

            if path.is_empty() {
                match store.list_packages() {
                    Ok(packages) => {
                        let mut output = format!("Packages in {}:\n\n", library_id);
                        for p in &packages {
                            output.push_str(&format!("  {}\n", p.name));
                            if let Some(ref doc) = p.doc_comment {
                                let short: String = doc.chars().take(100).collect();
                                output.push_str(&format!("    {}\n", short));
                            }
                        }
                        CallToolResult::success(vec![Content::text(output)])
                    }
                    Err(e) => CallToolResult::error(vec![Content::text(format!(
                        "Error: {}",
                        e
                    ))]),
                }
            } else if path.ends_with('.')
                || path.chars().last().is_some_and(|c| c.is_lowercase())
            {
                match store.list_types_in_package(&path) {
                    Ok(types) if types.is_empty() => {
                        browse_type_detail(&store, &path, &library_id)
                    }
                    Ok(types) => {
                        let mut output = format!("Types in {}:\n\n", path);
                        for t in &types {
                            output.push_str(&format!(
                                "  {} {} {}\n",
                                t.kind,
                                t.fqn,
                                t.superclass
                                    .as_deref()
                                    .map(|s| format!("extends {}", s))
                                    .unwrap_or_default()
                            ));
                            if let Some(ref doc) = t.doc_comment {
                                let short: String = doc.chars().take(100).collect();
                                output.push_str(&format!("    {}\n", short));
                            }
                        }
                        CallToolResult::success(vec![Content::text(output)])
                    }
                    Err(e) => CallToolResult::error(vec![Content::text(format!(
                        "Error: {}",
                        e
                    ))]),
                }
            } else {
                browse_type_detail(&store, &path, &library_id)
            }
        })
        .await;

        match result {
            Ok(r) => r,
            Err(e) => CallToolResult::error(vec![Content::text(format!("Task join error: {}", e))]),
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

            CallToolResult::success(vec![Content::text(output)])
        }
        Ok(None) => CallToolResult::error(vec![Content::text(format!(
            "Type '{}' not found in {}. Try browse_library with the package name to see available types.",
            fqn, library_id
        ))]),
        Err(e) => CallToolResult::error(vec![Content::text(format!("Error: {}", e))]),
    }
}
