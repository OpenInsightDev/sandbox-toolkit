use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::response::Response;
use axum::routing::{head, post};
use tokio::net::TcpListener;

use crate::handler::Handler;

/// Mounts the tus endpoints at the handler's configured base path.
pub fn router(handler: Arc<Handler>) -> Router {
    let base = handler.config().base_path.clone();

    Router::new()
        .merge(collection(&base))
        .merge(resource(&base))
        .with_state(handler)
}

/// Never resolves: like `axum::serve`, the future's output is uninhabited
/// without graceful shutdown.
pub async fn serve(listener: TcpListener, handler: Arc<Handler>) -> std::io::Result<()> {
    match axum::serve(listener, router(handler)).await {}
}

fn collection(base: &str) -> Router<Arc<Handler>> {
    let path = if base.is_empty() { "/" } else { base };
    Router::new().route(path, post(post_upload).options(options_upload))
}

fn resource(base: &str) -> Router<Arc<Handler>> {
    let path = format!("{base}/{{upload_id}}");
    Router::new().route(
        &path,
        head(head_upload)
            .patch(patch_upload)
            .delete(delete_upload)
            // `X-HTTP-Method-Override` tunnels PATCH/DELETE through POST.
            .post(post_upload),
    )
}

async fn options_upload(State(handler): State<Arc<Handler>>, request: Request) -> Response {
    handler.options(request).await
}

async fn post_upload(State(handler): State<Arc<Handler>>, request: Request) -> Response {
    handler.post(request).await
}

async fn head_upload(State(handler): State<Arc<Handler>>, request: Request) -> Response {
    handler.head(request).await
}

async fn patch_upload(State(handler): State<Arc<Handler>>, request: Request) -> Response {
    handler.patch(request).await
}

async fn delete_upload(State(handler): State<Arc<Handler>>, request: Request) -> Response {
    handler.delete(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::datastore::{BoxFuture, DataStore, FileInfo, StoreComposer, Upload};
    use crate::error::Result;

    struct StubStore;

    impl DataStore for StubStore {
        fn create_upload(&self, _info: FileInfo) -> BoxFuture<'_, Result<Box<dyn Upload>>> {
            todo!()
        }

        fn get_upload<'a>(&'a self, _id: &'a str) -> BoxFuture<'a, Result<Box<dyn Upload>>> {
            todo!()
        }
    }

    fn handler(config: Config) -> Arc<Handler> {
        let store = StoreComposer::new().with_core(Arc::new(StubStore));
        Arc::new(Handler::new(config, store).unwrap())
    }

    /// `Router::route` validates the path pattern at construction, so building
    /// the router is what proves the endpoints are mounted.
    #[test]
    fn mounts_endpoints_at_the_base_path() {
        let handler = handler(Config::default());
        assert_eq!(handler.extensions(), "creation,creation-with-upload");
        let _ = router(handler);
    }

    #[test]
    fn mounts_endpoints_at_the_root() {
        let config = Config {
            base_path: "/".to_owned(),
            ..Config::default()
        };
        let _ = router(handler(config));
    }
}
