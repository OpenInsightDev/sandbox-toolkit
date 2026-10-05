use std::io::ErrorKind;
use std::path::Path;

use salvo::http::StatusCode;
use salvo::prelude::*;

use super::http::{Target, decode, io_error};
use super::model::{FsError, RemoveRequest, TransferRequest};

pub(super) async fn dispatch(
    target: &Target,
    kind: &str,
    body: serde_json::Value,
    res: &mut Response,
) -> Result<(), StatusError> {
    match kind {
        "copy" => copy(target, decode(body)?, res).await,
        "move" => r#move(target, decode(body)?, res).await,
        other => Err(StatusError::bad_request().brief(format!("unknown type `{other}`"))),
    }
}

pub(super) async fn remove(
    target: &Target,
    body: serde_json::Value,
    res: &mut Response,
) -> Result<(), StatusError> {
    let request: RemoveRequest = decode(body)?;
    let path = target.resolve(&request.path);

    let removed = match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) if metadata.is_dir() && request.recursive.unwrap_or(false) => {
            tokio::fs::remove_dir_all(&path).await
        }
        Ok(metadata) if metadata.is_dir() => tokio::fs::remove_dir(&path).await,
        Ok(_) => tokio::fs::remove_file(&path).await,
        Err(error) => Err(error),
    };

    // `force` forgives a target that was never there, which is the same answer
    // whether the miss came from the probe above or from the removal itself.
    match removed {
        Ok(()) => {}
        Err(error) if request.force.unwrap_or(false) && error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(&target.render(&path), error).into()),
    }

    res.status_code(StatusCode::NO_CONTENT);
    Ok(())
}

async fn copy(
    target: &Target,
    request: TransferRequest,
    res: &mut Response,
) -> Result<(), StatusError> {
    let from = target.resolve(&request.path);
    let to = target.resolve(&request.destination);
    copy_onto(&from, &to).await?;

    res.status_code(StatusCode::NO_CONTENT);
    Ok(())
}

async fn r#move(
    target: &Target,
    request: TransferRequest,
    res: &mut Response,
) -> Result<(), StatusError> {
    let from = target.resolve(&request.path);
    let to = target.resolve(&request.destination);
    tokio::fs::rename(&from, &to)
        .await
        .map_err(|error| io_error(&target.render(&from), error))?;

    res.status_code(StatusCode::NO_CONTENT);
    Ok(())
}

/// Copies `from` onto `to`, replacing a file that is already there. A directory
/// source is recreated below `to` level by level, merging into a directory that
/// already exists. The frontier is walked rather than recursed so a deep tree
/// costs heap, not stack.
async fn copy_onto(from: &Path, to: &Path) -> Result<(), FsError> {
    let mut frontier = vec![(from.to_path_buf(), to.to_path_buf())];

    while let Some((from, to)) = frontier.pop() {
        let metadata = tokio::fs::metadata(&from)
            .await
            .map_err(|error| io_error(&from.to_string_lossy(), error))?;

        if !metadata.is_dir() {
            tokio::fs::copy(&from, &to)
                .await
                .map_err(|error| io_error(&to.to_string_lossy(), error))?;
            continue;
        }

        tokio::fs::create_dir_all(&to)
            .await
            .map_err(|error| io_error(&to.to_string_lossy(), error))?;
        let mut entries = tokio::fs::read_dir(&from)
            .await
            .map_err(|error| io_error(&from.to_string_lossy(), error))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| io_error(&from.to_string_lossy(), error))?
        {
            frontier.push((entry.path(), to.join(entry.file_name())));
        }
    }

    Ok(())
}
