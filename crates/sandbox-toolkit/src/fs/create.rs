use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use salvo::http::StatusCode;
use salvo::prelude::*;

use super::http::{Target, decode, io_error};
use super::model::{
    FsError, MakeDirectoryRequest, SymlinkRequest, TempKind, TempPath, TempRequest, WriteRequest,
};

/// The name a temp entry gets when the caller names none.
const TEMP_PREFIX: &str = "effect";

/// The characters a random temp name draws from.
const TEMP_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// How many random characters separate two callers' names.
const TEMP_RANDOM: usize = 6;

/// How many names a temp entry tries before a run of collisions is called off.
const TEMP_ATTEMPTS: usize = 16;

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
        // A temp entry lives outside the mount's addressing, so it takes no target.
        "temp" => temp(decode(body)?, res).await,
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

async fn temp(request: TempRequest, res: &mut Response) -> Result<(), StatusError> {
    let base = base_directory(request.directory.as_deref())?;
    let path = create_temp(&base, &request).await?;

    res.render(Json(TempPath {
        path: path.to_string_lossy().into_owned(),
    }));

    Ok(())
}

/// The mount's addressing does not reach a temp entry, so the base is the
/// caller's absolute directory or the server's own temp directory.
fn base_directory(directory: Option<&str>) -> Result<PathBuf, FsError> {
    match directory {
        None => Ok(std::env::temp_dir()),
        Some(directory) if Path::new(directory).is_absolute() => Ok(PathBuf::from(directory)),
        Some(directory) => Err(FsError::Invalid(format!(
            "`{directory}` is not an absolute directory"
        ))),
    }
}

async fn create_temp(base: &Path, request: &TempRequest) -> Result<PathBuf, FsError> {
    let prefix = request.prefix.as_deref().unwrap_or(TEMP_PREFIX);
    let suffix = request.suffix.as_deref().unwrap_or("");

    for _ in 0..TEMP_ATTEMPTS {
        let path = base.join(format!("{prefix}{}{suffix}", scramble()?));
        let created = match request.kind {
            TempKind::Directory => tokio::fs::DirBuilder::new().mode(0o700).create(&path).await,
            TempKind::File => tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .await
                .map(|_| ()),
        };

        match created {
            Ok(()) => return Ok(path),
            // A name already in use is the one failure a fresh name fixes.
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(io_error(&path.to_string_lossy(), error)),
        }
    }

    Err(FsError::Invalid(format!(
        "no free temp name in `{}`",
        base.display()
    )))
}

/// The random middle of a temp name, read from the kernel rather than derived
/// from a clock or a counter so a caller cannot guess a name in advance.
fn scramble() -> std::io::Result<String> {
    let mut random = [0u8; TEMP_RANDOM];
    let mut filled = 0;
    while filled < random.len() {
        // SAFETY: the pointer addresses `random[filled..]`, a live mutable slice
        // of exactly the length handed over.
        let read = unsafe {
            libc::getrandom(
                random[filled..].as_mut_ptr().cast(),
                random.len() - filled,
                0,
            )
        };
        if read <= 0 {
            return Err(std::io::Error::last_os_error());
        }
        filled += read as usize;
    }

    Ok(random
        .iter()
        .map(|byte| TEMP_ALPHABET[*byte as usize % TEMP_ALPHABET.len()] as char)
        .collect())
}
