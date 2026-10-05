use salvo::http::StatusCode;
use salvo::prelude::*;

use super::http::{Target, decode, io_error};
use super::model::{MakeDirectoryRequest, SymlinkRequest, WriteRequest};

pub(super) async fn dispatch(
    target: &Target,
    kind: &str,
    body: serde_json::Value,
    res: &mut Response,
) -> Result<(), StatusError> {
    match kind {
        "content" => content(target, decode(body)?, res).await,
        "directory" => directory(target, decode(body)?, res).await,
        "symlink" => symlink(target, decode(body)?, res).await,
        other => Err(StatusError::bad_request().brief(format!("unknown type `{other}`"))),
    }
}

async fn content(
    target: &Target,
    request: WriteRequest,
    res: &mut Response,
) -> Result<(), StatusError> {
    let path = target.resolve(&request.path);
    tokio::fs::write(&path, request.content.as_bytes())
        .await
        .map_err(|error| io_error(&target.render(&path), error))?;

    res.status_code(StatusCode::NO_CONTENT);
    Ok(())
}

async fn directory(
    target: &Target,
    request: MakeDirectoryRequest,
    res: &mut Response,
) -> Result<(), StatusError> {
    let path = target.resolve(&request.path);
    // Without `recursive` a missing parent is the caller's to fix, so the
    // plain `create_dir` failure is what surfaces as `404`.
    let created = if request.recursive.unwrap_or(false) {
        tokio::fs::create_dir_all(&path).await
    } else {
        tokio::fs::create_dir(&path).await
    };
    created.map_err(|error| io_error(&target.render(&path), error))?;

    res.status_code(StatusCode::NO_CONTENT);
    Ok(())
}

async fn symlink(
    target: &Target,
    request: SymlinkRequest,
    res: &mut Response,
) -> Result<(), StatusError> {
    let path = target.resolve(&request.path);
    // The target is written verbatim, so a relative one stays relative and need
    // not exist.
    tokio::fs::symlink(&request.target, &path)
        .await
        .map_err(|error| io_error(&target.render(&path), error))?;

    res.status_code(StatusCode::NO_CONTENT);
    Ok(())
}
