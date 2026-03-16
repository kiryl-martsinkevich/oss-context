use crate::mcp::{self, JsonRpcRequest, JsonRpcResponse};
use crate::tools::OssContextServer;
use anyhow::Result;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

async fn handle_request(server: &OssContextServer, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
    match req.method.as_str() {
        "initialize" => Some(JsonRpcResponse::success(req.id, mcp::initialize_result())),
        "initialized" | "notifications/initialized" => None,
        "ping" => Some(JsonRpcResponse::success(req.id, serde_json::json!({}))),
        "tools/list" => {
            let tools = OssContextServer::tool_definitions();
            Some(JsonRpcResponse::success(
                req.id,
                serde_json::json!({ "tools": tools }),
            ))
        }
        "tools/call" => {
            let params = req.params.unwrap_or(Value::Null);
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or(Value::Object(Default::default()));

            if name.is_empty() {
                return Some(JsonRpcResponse::invalid_params(
                    req.id,
                    "Missing 'name' in tools/call params".to_string(),
                ));
            }

            let result = server.call_tool(name, arguments).await;
            Some(JsonRpcResponse::success(
                req.id,
                serde_json::to_value(&result).unwrap_or(Value::Null),
            ))
        }
        _ => {
            if req.id.is_none() {
                None
            } else {
                Some(JsonRpcResponse::method_not_found(req.id, &req.method))
            }
        }
    }
}

pub async fn serve_stdio(server: OssContextServer) -> Result<()> {
    tracing::info!("Starting MCP server on stdio");

    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let reader = BufReader::new(stdin);
    let mut lines = reader.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let req: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = JsonRpcResponse::error(
                    None,
                    -32700,
                    format!("Parse error: {}", e),
                );
                let out = serde_json::to_string(&resp).unwrap();
                stdout.write_all(out.as_bytes()).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
                continue;
            }
        };

        if let Some(resp) = handle_request(&server, req).await {
            let out = serde_json::to_string(&resp).unwrap();
            stdout.write_all(out.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }

    Ok(())
}

pub async fn serve_sse(server: OssContextServer, port: u16) -> Result<()> {
    use axum::extract::State;
    use axum::response::sse::{Event, Sse};
    use axum::response::IntoResponse;
    use axum::routing::post;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    struct AppState {
        server: OssContextServer,
        tx: broadcast::Sender<String>,
    }

    let (tx, _) = broadcast::channel::<String>(256);

    let state = Arc::new(AppState {
        server,
        tx,
    });

    let app = axum::Router::new()
        .route(
            "/mcp/sse",
            axum::routing::get({
                let state = state.clone();
                move || {
                    let mut rx = state.tx.subscribe();
                    async move {
                        let stream = async_stream::stream! {
                            while let Ok(msg) = rx.recv().await {
                                yield Ok::<_, std::convert::Infallible>(
                                    Event::default().event("message").data(msg)
                                );
                            }
                        };
                        Sse::new(stream)
                    }
                }
            }),
        )
        .route(
            "/mcp",
            post(
                |State(state): State<Arc<AppState>>, body: String| async move {
                    let req: JsonRpcRequest = match serde_json::from_str(&body) {
                        Ok(r) => r,
                        Err(e) => {
                            let resp = JsonRpcResponse::error(
                                None,
                                -32700,
                                format!("Parse error: {}", e),
                            );
                            return axum::Json(resp).into_response();
                        }
                    };

                    if let Some(resp) = handle_request(&state.server, req).await {
                        let json_str = serde_json::to_string(&resp).unwrap();
                        let _ = state.tx.send(json_str);
                        axum::Json(resp).into_response()
                    } else {
                        axum::http::StatusCode::ACCEPTED.into_response()
                    }
                },
            ),
        )
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    tracing::info!("MCP server listening on http://{}/mcp", addr);
    axum::serve(listener, app).await?;

    Ok(())
}
