use crate::tools::OssContextServer;
use anyhow::Result;
use rmcp::ServiceExt;

pub async fn serve_stdio(server: OssContextServer) -> Result<()> {
    tracing::info!("Starting MCP server on stdio");
    let service = server.serve(rmcp::transport::io::stdio()).await?;
    service.waiting().await?;
    Ok(())
}

pub async fn serve_sse(server: OssContextServer, port: u16) -> Result<()> {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService,
    };
    use std::sync::Arc;

    let ct = tokio_util::sync::CancellationToken::new();
    let config = StreamableHttpServerConfig {
        stateful_mode: true,
        cancellation_token: ct.clone(),
        ..Default::default()
    };

    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default()),
        config,
    );

    let router = axum::Router::new().nest_service("/mcp", service);
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    tracing::info!("MCP server listening on http://{}/mcp", addr);
    axum::serve(listener, router)
        .with_graceful_shutdown(async move { ct.cancelled_owned().await })
        .await?;

    Ok(())
}
