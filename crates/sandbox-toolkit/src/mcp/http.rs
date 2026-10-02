use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{FromRequestParts, OriginalUri, Path, Request};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;
use tower::Service;

use super::model::{StreamableHttpMcpServer, StreamableHttpServerMcpConfig};
use super::runtime;

pub type ExtractRuntime = Arc<runtime::Runtime>;

/// Named rather than positional: sibling captures such as `{workspace_id}` on the
/// enclosing mount land in the same set, and a tuple would fail on the count.
#[derive(Debug, Deserialize)]
struct McpPath {
    mcp_id: String,
}

pub fn routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
    ExtractRuntime: FromRequestParts<S>,
{
    Router::new()
        .route("/", get(list))
        .route("/{mcp_id}", get(proxy).post(proxy).delete(proxy))
}

async fn list(
    runtime: ExtractRuntime,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Response {
    let base = format!("{}{}", origin(&uri, &headers), uri.path());
    let servers = runtime
        .ids()
        .await
        .into_iter()
        .map(|id| {
            let url = format!("{base}/{id}");
            (id, StreamableHttpMcpServer::new(url))
        })
        .collect();

    Json(StreamableHttpServerMcpConfig::new(servers)).into_response()
}

async fn proxy(runtime: ExtractRuntime, Path(path): Path<McpPath>, request: Request) -> Response {
    let Some(mut service) = runtime.get(&path.mcp_id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    match service.call(request).await {
        Ok(response) => response.map(Body::new),
        Err(never) => match never {},
    }
}

/// The external origin (`scheme` + host) the request reached us through, resolved
/// like axum's `Scheme`/`Host` extractors: a forwarded header, then the request
/// URI (h2 carries scheme and authority there; an h1 origin-form request only the
/// path, so `Host` stands in).
fn origin(uri: &Uri, headers: &HeaderMap) -> String {
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .or_else(|| uri.scheme_str())
        .unwrap_or("http");
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| uri.authority().map(|authority| authority.as_str()))
        .unwrap_or_default();

    format!("{scheme}://{host}")
}
