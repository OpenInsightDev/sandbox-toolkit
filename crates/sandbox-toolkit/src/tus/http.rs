use salvo::http::StatusCode;
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
    let request = req
        .strip_to_hyper::<hyper::body::Incoming>()
        .map_err(|_| StatusError::internal_server_error())?;

    match state.tus.forward(origin_form(request)).await {
        Ok(response) => {
            res.merge_hyper(response);

            Ok(())
        }
        Err(error) => {
            tracing::debug!(%error, "the tus sidecar is unreachable");

            Err(StatusError::from_code(StatusCode::BAD_GATEWAY)
                .unwrap_or_else(StatusError::internal_server_error))
        }
    }
}

/// The path and query of `request`, which is what an origin-form request line
/// carries upstream.
fn origin_form(mut request: hyper::Request<hyper::body::Incoming>) -> hyper::Request<hyper::body::Incoming> {
    let parts = request.uri().clone().into_parts();
    let uri = hyper::Uri::from_parts(parts).expect("a path taken from a URI is a valid URI");
    *request.uri_mut() = uri;

    request
}
