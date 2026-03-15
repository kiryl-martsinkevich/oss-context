use crate::config::AppConfig;
use crate::parser;
use crate::resolver;
use crate::store::{LibraryId, Store};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ErrorData, ServerHandler};
use schemars::JsonSchema;
use serde::Deserialize;
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
    tool_router: ToolRouter<Self>,
}

impl OssContextServer {
    pub fn new(config: AppConfig) -> Self {
        Self {
            config: Arc::new(config),
            tool_router: Self::tool_router(),
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
}

#[tool_router(router = tool_router)]
impl OssContextServer {
    #[tool(description = "Resolve a Java library and make its documentation available for querying. Accepts 'groupId:artifactId:version' format. Returns the library ID to use with query_docs and browse_library.")]
    async fn resolve_library(
        &self,
        params: Parameters<ResolveLibraryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let query = params.0.query.trim().to_string();
        info!("Resolving library: {}", query);

        let lib = match Self::parse_library_id(&query) {
            Some(lib) => lib,
            None => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Could not parse library identifier '{}'. Please use groupId:artifactId:version format (e.g. 'com.google.guava:guava:33.0.0-jre')",
                    query
                ))]));
            }
        };

        // Check if already indexed
        if let Some(store) = self.open_store(&lib) {
            if store.has_library_meta().unwrap_or(false) {
                return Ok(CallToolResult::success(vec![Content::text(format!(
                    "Library {} is ready. Use this ID with query_docs and browse_library: {}",
                    lib.to_string_id(),
                    lib.to_string_id()
                ))]));
            }
        }

        // Resolve and index
        match self.resolve_and_index(&lib) {
            Ok(_) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Library {} indexed successfully. Use this ID with query_docs and browse_library: {}",
                lib.to_string_id(),
                lib.to_string_id()
            ))])),
            Err(e) => {
                error!("Failed to resolve {}: {}", lib.to_string_id(), e);
                Ok(CallToolResult::error(vec![Content::text(format!(
                    "Could not find documentation for {}: {}",
                    lib.to_string_id(),
                    e
                ))]))
            }
        }
    }

    #[tool(description = "Search documentation across a resolved Java library using free-text queries. Returns matching classes, methods, and fields ranked by relevance.")]
    async fn query_docs(
        &self,
        params: Parameters<QueryDocsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let lib = match Self::parse_library_id(&params.0.library_id) {
            Some(lib) => lib,
            None => {
                return Ok(CallToolResult::error(vec![Content::text(
                    "Invalid library_id format. Use groupId:artifactId:version",
                )]));
            }
        };

        let store = match self.open_store(&lib) {
            Some(store) => store,
            None => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Library {} is not indexed. Call resolve_library first.",
                    params.0.library_id
                ))]));
            }
        };

        let limit = params.0.limit.unwrap_or(20);
        match store.search(&params.0.query, limit) {
            Ok(results) if results.is_empty() => Ok(CallToolResult::success(vec![Content::text(
                format!(
                    "No results found for '{}' in {}",
                    params.0.query, params.0.library_id
                ),
            )])),
            Ok(results) => {
                let mut output = format!(
                    "Found {} results for '{}' in {}:\n\n",
                    results.len(),
                    params.0.query,
                    params.0.library_id
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
                Ok(CallToolResult::success(vec![Content::text(output)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Search error: {}",
                e
            ))])),
        }
    }

    #[tool(description = "Browse a Java library's documentation hierarchically. Without a path: list packages. With a package name: list classes. With a fully qualified class name: show full documentation including all methods and fields.")]
    async fn browse_library(
        &self,
        params: Parameters<BrowseLibraryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let lib = match Self::parse_library_id(&params.0.library_id) {
            Some(lib) => lib,
            None => {
                return Ok(CallToolResult::error(vec![Content::text(
                    "Invalid library_id format.",
                )]));
            }
        };

        let store = match self.open_store(&lib) {
            Some(store) => store,
            None => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Library {} is not indexed. Call resolve_library first.",
                    params.0.library_id
                ))]));
            }
        };

        let path = params.0.path.as_deref().unwrap_or("");

        if path.is_empty() {
            match store.list_packages() {
                Ok(packages) => {
                    let mut output = format!("Packages in {}:\n\n", params.0.library_id);
                    for p in &packages {
                        output.push_str(&format!("  {}\n", p.name));
                        if let Some(ref doc) = p.doc_comment {
                            let short: String = doc.chars().take(100).collect();
                            output.push_str(&format!("    {}\n", short));
                        }
                    }
                    Ok(CallToolResult::success(vec![Content::text(output)]))
                }
                Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                    "Error: {}",
                    e
                ))])),
            }
        } else if path
            .chars()
            .last()
            .map_or(true, |c| c.is_lowercase() || c == '.')
        {
            match store.list_types_in_package(path) {
                Ok(types) if types.is_empty() => {
                    Ok(browse_type_detail(&store, path, &params.0.library_id))
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
                    Ok(CallToolResult::success(vec![Content::text(output)]))
                }
                Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                    "Error: {}",
                    e
                ))])),
            }
        } else {
            Ok(browse_type_detail(&store, path, &params.0.library_id))
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

#[tool_handler(router = self.tool_router)]
impl ServerHandler for OssContextServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            Implementation::new("oss-context", env!("CARGO_PKG_VERSION")),
        )
    }
}
