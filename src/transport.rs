use crate::tools::OssContextServer;
use anyhow::Result;
use rmcp::ServiceExt;

pub async fn serve_stdio(server: OssContextServer) -> Result<()> {
    tracing::info!("Starting MCP server on stdio");
    let service = server.serve(rmcp::transport::io::stdio()).await?;
    service.waiting().await?;
    Ok(())
}

pub async fn serve_sse(_server: OssContextServer, port: u16) -> Result<()> {
    tracing::info!("Starting MCP server on SSE port {}", port);
    // SSE/HTTP transport requires additional setup with axum
    // For now, this is a placeholder — stdio is the primary transport
    todo!("SSE transport - requires axum integration with rmcp's server-side-http feature")
}
