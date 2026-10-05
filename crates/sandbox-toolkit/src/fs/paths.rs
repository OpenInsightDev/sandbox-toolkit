use salvo::http::StatusCode;
use salvo::prelude::*;

use super::http::Target;

pub(super) async fn dispatch(
    _target: &Target,
    kind: &str,
    _body: serde_json::Value,
    _res: &mut Response,
) -> Result<(), StatusError> {
    Err(unimplemented(kind))
}

pub(super) async fn remove(
    _target: &Target,
    _body: serde_json::Value,
    _res: &mut Response,
) -> Result<(), StatusError> {
    Err(unimplemented("remove"))
}

fn unimplemented(kind: &str) -> StatusError {
    StatusError::from_code(StatusCode::NOT_IMPLEMENTED)
        .unwrap_or_else(StatusError::internal_server_error)
        .brief(format!("`{kind}` is not implemented"))
}
