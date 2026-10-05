use salvo::http::body::ResBody;
use salvo::http::uri::Uri;
use salvo::http::{ReqBody, StatusCode};
use salvo::prelude::*;

use crate::http::app_state;

/// The mount and everything below it are the same proxy, so the upload URL tusd
/// answers with is reachable where the client was told it is.
pub fn routes() -> Router {
    Router::new()
        .goal(proxy)
        .push(Router::with_path("{**rest}").goal(proxy))
}

#[handler]
async fn proxy(req: &mut Request, depot: &mut Depot, res: &mut Response) -> Result<(), StatusError> {
    let state = app_state(depot)?;

    match state.tus.forward(upstream_request(req)).await {
        Ok(response) => {
            let (parts, body) = response.into_parts();
            *res.version_mut() = parts.version;
            res.status_code(parts.status);
            res.set_headers(parts.headers);
            res.body(ResBody::from(body));

            Ok(())
        }
        Err(error) => {
            tracing::debug!(%error, "the tus sidecar is unreachable");

            Err(StatusError::from_code(StatusCode::BAD_GATEWAY)
                .unwrap_or_else(StatusError::internal_server_error))
        }
    }
}

/// The request tusd is reached with: the caller's method, version and headers,
/// its body still streaming, under the URI an origin-form request line carries.
fn upstream_request(req: &mut Request) -> hyper::Request<ReqBody> {
    let body = req.take_body();
    let mut request = hyper::Request::new(body);
    *request.method_mut() = req.method().clone();
    *request.uri_mut() = origin_form(req.uri());
    *request.version_mut() = req.version();
    *request.headers_mut() = std::mem::take(req.headers_mut());

    request
}

/// The path and query of `uri`, dropping the scheme and authority a request line
/// does not carry.
fn origin_form(uri: &Uri) -> Uri {
    let mut parts = uri.clone().into_parts();
    parts.scheme = None;
    parts.authority = None;

    Uri::from_parts(parts).expect("a path taken from a URI is a valid URI")
}
