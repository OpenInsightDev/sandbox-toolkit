use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use salvo::http::StatusCode;
use salvo::prelude::*;

use super::http::{Target, decode, io_error, read};
use super::model::{FsError, MetadataRequest, TextPatchRequest, TruncateRequest};

pub(super) async fn dispatch(
    target: &Target,
    kind: &str,
    body: serde_json::Value,
    res: &mut Response,
) -> Result<(), StatusError> {
    match kind {
        "metadata" => metadata(target, decode(body)?, res),
        "patch" => patch(target, decode(body)?, res).await,
        "truncate" => truncate(target, decode(body)?, res).await,
        other => Err(StatusError::bad_request().brief(format!("unknown type `{other}`"))),
    }
}

/// Rewrites only the fields the request carries, leaving every other attribute
/// as it is.
fn metadata(
    target: &Target,
    request: MetadataRequest,
    res: &mut Response,
) -> Result<(), StatusError> {
    let path = target.resolve(&request.path);
    let rendered = target.render(&path);
    let name = c_path(&path, &rendered)?;

    if let Some(mode) = request.mode.as_deref() {
        let bits = u32::from_str_radix(mode, 8)
            .map_err(|_| FsError::Invalid(format!("`{mode}` is not an octal mode")))?;

        // `chmod(2)` follows a symlink to the file it names.
        // SAFETY: `name` is a valid NUL-terminated path, and `bits` holds a
        // mode the caller asked to apply.
        if unsafe { libc::chmod(name.as_ptr(), bits as libc::mode_t) } == -1 {
            return Err(io_error(&rendered, std::io::Error::last_os_error()).into());
        }
    }

    if request.uid.is_some() || request.gid.is_some() {
        // `-1` in either slot is the documented "leave that owner unchanged".
        let uid = request.uid.unwrap_or(u32::MAX);
        let gid = request.gid.unwrap_or(u32::MAX);

        // SAFETY: `name` is a valid NUL-terminated path, and `uid`/`gid` are
        // either an id to set or the `-1` sentinel.
        if unsafe { libc::chown(name.as_ptr(), uid, gid) } == -1 {
            return Err(io_error(&rendered, std::io::Error::last_os_error()).into());
        }
    }

    if request.atime.is_some() || request.mtime.is_some() {
        let times = [
            timestamp(request.atime.as_deref())?,
            timestamp(request.mtime.as_deref())?,
        ];

        // SAFETY: `name` is a valid NUL-terminated path and `times` holds two
        // live `timespec`s; a timestamp left out carries `UTIME_OMIT`.
        if unsafe { libc::utimensat(libc::AT_FDCWD, name.as_ptr(), times.as_ptr(), 0) } == -1 {
            return Err(io_error(&rendered, std::io::Error::last_os_error()).into());
        }
    }

    res.status_code(StatusCode::NO_CONTENT);
    Ok(())
}

async fn patch(
    target: &Target,
    request: TextPatchRequest,
    res: &mut Response,
) -> Result<(), StatusError> {
    if request.format != "unified" {
        return Err(FsError::Invalid(format!("unknown patch format `{}`", request.format)).into());
    }

    let path = target.resolve(&request.path);
    let rendered = target.render(&path);
    let bytes = read(&path).await?;
    let content = String::from_utf8(bytes).map_err(|_| FsError::NotUtf8(rendered.clone()))?;

    let patch = diffy::Patch::from_str(&request.patch)
        .map_err(|error| FsError::Invalid(format!("the patch is not a unified diff: {error}")))?;
    let patched = diffy::apply(&content, &patch)
        .map_err(|error| FsError::Invalid(format!("the patch does not apply: {error}")))?;

    tokio::fs::write(&path, patched)
        .await
        .map_err(|error| io_error(&rendered, error))?;

    res.status_code(StatusCode::NO_CONTENT);
    Ok(())
}

/// Truncates the file to `length`, which defaults to zero.
async fn truncate(
    target: &Target,
    request: TruncateRequest,
    res: &mut Response,
) -> Result<(), StatusError> {
    let path = target.resolve(&request.path);
    let rendered = target.render(&path);
    let length = request.length.unwrap_or(0);

    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .await
        .map_err(|error| io_error(&rendered, error))?;

    // A `length` past the current size grows the file with zero bytes, which the
    // endpoint treats as success.
    file.set_len(length)
        .await
        .map_err(|error| io_error(&rendered, error))?;

    res.status_code(StatusCode::NO_CONTENT);
    Ok(())
}

fn c_path(path: &Path, rendered: &str) -> Result<CString, FsError> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| FsError::Invalid(format!("`{rendered}` is not a path")))
}

/// An RFC 3339 timestamp in the encoding `QUERY ?type=metadata` writes; `None`
/// asks `utimensat(2)` to leave that timestamp alone.
fn timestamp(iso: Option<&str>) -> Result<libc::timespec, FsError> {
    let Some(iso) = iso else {
        return Ok(libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_OMIT,
        });
    };

    let at = chrono::DateTime::parse_from_rfc3339(iso)
        .map_err(|_| FsError::Invalid(format!("`{iso}` is not an RFC 3339 timestamp")))?;

    Ok(libc::timespec {
        tv_sec: at.timestamp(),
        tv_nsec: at.timestamp_subsec_nanos() as libc::c_long,
    })
}
