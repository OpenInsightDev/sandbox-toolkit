use std::collections::VecDeque;
use std::convert::Infallible;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use bytes::Bytes;
use futures_util::stream;
use globset::{GlobBuilder, GlobMatcher};
use notify::EventKind;
use salvo::extract::JsonBody;
use salvo::http::body::ResBody;
use salvo::http::header::HeaderValue;
use salvo::http::{Method, StatusCode, header};
use salvo::prelude::*;
use sandbox_toolkit_utils::watch::{Watch, WatchEvent};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::io::ReaderStream;

use crate::http::ExtractWorkspace;

use super::model::{
    AccessRequest, Change, Content, Depth, Entries, FsError, GlobRequest, Infinite, Lines,
    LinesRequest, ListRequest, PathRequest, RealPath, Stat, WatchRequest,
};
use super::{create, modify, paths};

/// The page a `lines` read returns when the request names none.
const DEFAULT_LINES: u64 = 1000;

/// Whether the mount names a workspace or is the unscoped global one, which also
/// decides how a request path and an entry path are written.
#[derive(Debug, Clone, Copy)]
pub(super) enum Scope {
    Global,
    Workspace,
}

/// The addressing a request and its response paths share: a workspace resolves a
/// request path against its root, the global mount takes it as it is.
#[derive(Debug, Clone)]
pub(super) struct Target {
    root: PathBuf,
    scope: Scope,
}

impl Target {
    fn new(root: PathBuf, scope: Scope) -> Self {
        Self { root, scope }
    }

    pub(super) fn resolve(&self, path: &str) -> PathBuf {
        if path.is_empty() {
            return self.root.clone();
        }

        match self.scope {
            Scope::Workspace => self.root.join(path),
            Scope::Global => PathBuf::from(path),
        }
    }

    pub(super) fn render(&self, path: &Path) -> String {
        match self.scope {
            Scope::Workspace => path
                .strip_prefix(&self.root)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned(),
            Scope::Global => path.to_string_lossy().into_owned(),
        }
    }
}

pub fn routes() -> Router {
    Router::new()
        .query(handle)
        .put(handle)
        .patch(handle)
        .post(handle)
        .delete(handle)
}

#[handler]
async fn handle(
    workspace: ExtractWorkspace,
    body: JsonBody<serde_json::Value>,
    req: &mut Request,
    res: &mut Response,
) -> Result<(), StatusError> {
    // A mount carrying `{workspace_id}` addresses the workspace root; the
    // unscoped mount addresses absolute paths.
    let scope = match req.param::<String>("workspace_id") {
        Some(_) => Scope::Workspace,
        None => Scope::Global,
    };
    let target = Target::new(workspace.metadata().await.root, scope);

    // `DELETE` is the one write whose method alone names the operation, so it is
    // the one request that carries no `type`.
    let method = req.method().clone();
    if method == Method::DELETE {
        return paths::remove(&target, body.0, res).await;
    }

    let kind = req
        .query::<String>("type")
        .ok_or_else(|| StatusError::bad_request().brief("`type` is required"))?;

    match method {
        Method::QUERY => dispatch(&target, &kind, body.0, res).await,
        Method::PUT => create::dispatch(&target, &kind, body.0, res).await,
        Method::PATCH => modify::dispatch(&target, &kind, body.0, res).await,
        Method::POST => paths::dispatch(&target, &kind, body.0, res).await,
        _ => Err(StatusError::method_not_allowed()),
    }
}

async fn dispatch(
    target: &Target,
    kind: &str,
    body: serde_json::Value,
    res: &mut Response,
) -> Result<(), StatusError> {
    match kind {
        "content" => content(target, decode(body)?, res).await,
        "stream" => stream_file(target, decode(body)?, res).await,
        "metadata" => stat(target, decode(body)?, res).await,
        "list" => list(target, decode(body)?, res).await,
        "glob" => glob(target, decode(body)?, res).await,
        "realpath" => realpath(target, decode(body)?, res).await,
        "access" => access(target, decode(body)?, res).await,
        "lines" => lines(target, decode(body)?, res).await,
        "watch" => watch(target, decode(body)?, res).await,
        other => Err(StatusError::bad_request().brief(format!("unknown type `{other}`"))),
    }
}

pub(super) fn decode<T: serde::de::DeserializeOwned>(
    body: serde_json::Value,
) -> Result<T, FsError> {
    serde_json::from_value(body).map_err(|error| FsError::Invalid(error.to_string()))
}

async fn content(
    target: &Target,
    request: PathRequest,
    res: &mut Response,
) -> Result<(), StatusError> {
    let path = target.resolve(&request.path);
    let bytes = read(&path).await?;
    let size = bytes.len() as u64;
    let content = String::from_utf8(bytes).map_err(|_| FsError::NotUtf8(target.render(&path)))?;

    res.render(Json(Content {
        path: target.render(&path),
        content,
        size,
    }));

    Ok(())
}

async fn stream_file(
    target: &Target,
    request: PathRequest,
    res: &mut Response,
) -> Result<(), StatusError> {
    let path = target.resolve(&request.path);
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|error| io_error(&target.render(&path), error))?;

    res.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(OCTET_STREAM));
    res.body(ResBody::stream(ReaderStream::new(file)));

    Ok(())
}

const OCTET_STREAM: &str = "application/octet-stream";
const NDJSON: &str = "application/x-ndjson";

async fn stat(
    target: &Target,
    request: PathRequest,
    res: &mut Response,
) -> Result<(), StatusError> {
    let path = target.resolve(&request.path);
    let metadata = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|error| io_error(&target.render(&path), error))?;

    res.render(Json(Stat::new(&metadata)));

    Ok(())
}

async fn list(
    target: &Target,
    request: ListRequest,
    res: &mut Response,
) -> Result<(), StatusError> {
    let root = target.resolve(&request.path);
    let depth = request.depth.unwrap_or(Depth::Count(1));
    let mut found = walk(&root, depth).await?;
    found.sort();

    let entries = page(&found, target, request.offset, request.limit);
    res.render(Json(entries));

    Ok(())
}

async fn glob(
    target: &Target,
    request: GlobRequest,
    res: &mut Response,
) -> Result<(), StatusError> {
    let root = target.resolve(&request.path);
    let pattern = glob_for(&request.pattern)?;
    let excludes = request
        .exclude
        .iter()
        .map(|exclude| glob_for(exclude))
        .collect::<Result<Vec<_>, _>>()?;

    let mut matched = Vec::new();
    for path in walk(&root, Depth::Infinity(Infinite::Yes)).await? {
        let relative = path.strip_prefix(&root).unwrap_or(&path);
        if pattern.is_match(relative) && !excluded(relative, &excludes) {
            matched.push(path);
        }
    }
    matched.sort();

    let entries = page(&matched, target, request.offset, request.limit);
    res.render(Json(entries));

    Ok(())
}

async fn realpath(
    target: &Target,
    request: PathRequest,
    res: &mut Response,
) -> Result<(), StatusError> {
    let path = target.resolve(&request.path);
    let resolved = tokio::fs::canonicalize(&path)
        .await
        .map_err(|error| io_error(&target.render(&path), error))?;

    res.render(Json(RealPath {
        path: resolved.to_string_lossy().into_owned(),
    }));

    Ok(())
}

async fn access(
    target: &Target,
    request: AccessRequest,
    res: &mut Response,
) -> Result<(), StatusError> {
    let path = target.resolve(&request.path);
    let mut mode = 0;
    if request.ok.unwrap_or(false) {
        mode |= libc::F_OK;
    }
    if request.readable.unwrap_or(false) {
        mode |= libc::R_OK;
    }
    if request.writable.unwrap_or(false) {
        mode |= libc::W_OK;
    }
    // No attribute was asked for, which leaves existence as the requirement.
    if mode == 0 {
        mode = libc::F_OK;
    }

    let rendered = target.render(&path);
    let name = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| FsError::Invalid(format!("`{rendered}` is not a path")))?;

    // `access(2)` probes the attributes against the serving process's ids.
    // SAFETY: `name` is a valid NUL-terminated path, and `mode` holds only
    // `F_OK`, `R_OK` and `W_OK`.
    if unsafe { libc::access(name.as_ptr(), mode) } == 0 {
        res.status_code(StatusCode::NO_CONTENT);
        return Ok(());
    }

    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ENOENT) => Err(FsError::NotFound(rendered).into()),
        Some(libc::EACCES) => Err(FsError::PermissionDenied(rendered).into()),
        _ => Err(FsError::Io(error).into()),
    }
}

async fn lines(
    target: &Target,
    request: LinesRequest,
    res: &mut Response,
) -> Result<(), StatusError> {
    let path = target.resolve(&request.path);
    let bytes = read(&path).await?;
    let text = String::from_utf8(bytes).map_err(|_| FsError::NotUtf8(target.render(&path)))?;

    let offset = request.offset.unwrap_or(0) as usize;
    let limit = request.limit.unwrap_or(DEFAULT_LINES) as usize;
    let all: Vec<&str> = text.lines().collect();
    let page: Vec<String> = all
        .iter()
        .skip(offset)
        .take(limit)
        .map(|line| (*line).to_owned())
        .collect();
    let truncated = all.len() > offset + page.len();

    res.render(Json(Lines {
        lines: page,
        truncated,
    }));

    Ok(())
}

async fn watch(
    target: &Target,
    request: WatchRequest,
    res: &mut Response,
) -> Result<(), StatusError> {
    let path = target.resolve(&request.path);
    let watch = if request.recursive.unwrap_or(false) {
        Watch::recursive(&path).await.map_err(FsError::Watch)?
    } else {
        Watch::new(&path).await.map_err(FsError::Watch)?
    };
    let events = watch.subscribe();

    let state = Watches {
        _watch: watch,
        events,
        target: target.clone(),
        pending: VecDeque::new(),
    };
    let events = stream::unfold(state, |mut state| async move {
        loop {
            if let Some(line) = state.pending.pop_front() {
                return Some((Ok::<_, Infallible>(line), state));
            }

            match state.events.recv().await {
                Ok(Ok(event)) => state.enqueue(event),
                Ok(Err(_)) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return None,
            }
        }
    });

    res.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(NDJSON));
    res.body(ResBody::stream(events));

    Ok(())
}

/// The state of one open `watch` stream. The watcher rides along so the debouncer
/// stops when the caller disconnects and the stream is dropped.
struct Watches {
    _watch: Watch,
    events: Receiver<WatchEvent>,
    target: Target,
    pending: VecDeque<Bytes>,
}

impl Watches {
    fn enqueue(&mut self, event: notify::Event) {
        let Some(name) = classify(&event.kind) else {
            return;
        };

        for path in event.paths {
            let change = Change {
                event: name,
                path: self.target.render(&path),
            };
            let Ok(mut line) = serde_json::to_vec(&change) else {
                continue;
            };
            line.push(b'\n');
            self.pending.push_back(Bytes::from(line));
        }
    }
}

fn classify(kind: &EventKind) -> Option<&'static str> {
    match kind {
        EventKind::Create(_) => Some("create"),
        EventKind::Modify(_) => Some("update"),
        EventKind::Remove(_) => Some("remove"),
        EventKind::Access(_) | EventKind::Any | EventKind::Other => None,
    }
}

/// Collects the entries below `root`, up to `depth` levels. A directory is
/// descended into through its own entry, so a symlink is reported once and not
/// followed.
async fn walk(root: &Path, depth: Depth) -> Result<Vec<PathBuf>, FsError> {
    let mut found = Vec::new();
    let mut frontier = vec![root.to_path_buf()];
    let mut level = 0u64;

    loop {
        if let Depth::Count(limit) = depth
            && level >= limit
        {
            break;
        }

        let mut next = Vec::new();
        for directory in &frontier {
            let mut entries = tokio::fs::read_dir(directory)
                .await
                .map_err(|error| io_error(&directory.to_string_lossy(), error))?;

            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|error| io_error(&directory.to_string_lossy(), error))?
            {
                let path = entry.path();
                let file_type = entry
                    .file_type()
                    .await
                    .map_err(|error| io_error(&path.to_string_lossy(), error))?;
                if file_type.is_dir() {
                    next.push(path.clone());
                }
                found.push(path);
            }
        }

        if next.is_empty() {
            break;
        }
        frontier = next;
        level += 1;
    }

    Ok(found)
}

fn page(paths: &[PathBuf], target: &Target, offset: Option<u64>, limit: Option<u64>) -> Entries {
    let offset = offset.unwrap_or(0) as usize;
    let limit = limit.map_or(usize::MAX, |limit| limit as usize);

    let rendered = paths
        .iter()
        .skip(offset)
        .take(limit)
        .map(|path| target.render(path))
        .collect();

    Entries::new(rendered)
}

/// `*` stays within one path component, so `**` is what crosses a separator.
fn glob_for(pattern: &str) -> Result<GlobMatcher, FsError> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|error| FsError::Invalid(format!("invalid pattern `{pattern}`: {error}")))
}

fn excluded(relative: &Path, excludes: &[GlobMatcher]) -> bool {
    if excludes.is_empty() {
        return false;
    }

    // A pattern naming a directory excludes everything below it.
    relative
        .ancestors()
        .take_while(|ancestor| !ancestor.as_os_str().is_empty())
        .any(|ancestor| excludes.iter().any(|exclude| exclude.is_match(ancestor)))
}

pub(super) async fn read(path: &Path) -> Result<Vec<u8>, FsError> {
    tokio::fs::read(path)
        .await
        .map_err(|error| io_error(&path.to_string_lossy(), error))
}

pub(super) fn io_error(path: &str, error: std::io::Error) -> FsError {
    match error.kind() {
        std::io::ErrorKind::NotFound => FsError::NotFound(path.to_owned()),
        std::io::ErrorKind::PermissionDenied => FsError::PermissionDenied(path.to_owned()),
        // `EEXIST` and `ENOTEMPTY` both mean the state on disk forbids the write,
        // which is the same answer to the caller either way.
        std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::DirectoryNotEmpty => {
            FsError::Conflict(path.to_owned())
        }
        _ => FsError::Io(error),
    }
}

/// A path the caller asked about is the caller's to fix, so its failure reads as
/// the status that names it.
impl From<FsError> for StatusError {
    fn from(error: FsError) -> Self {
        let status = match &error {
            FsError::NotFound(_) => StatusCode::NOT_FOUND,
            FsError::PermissionDenied(_) => StatusCode::FORBIDDEN,
            FsError::Conflict(_) => StatusCode::CONFLICT,
            FsError::NotUtf8(_) => StatusCode::UNPROCESSABLE_ENTITY,
            FsError::Invalid(_) => StatusCode::BAD_REQUEST,
            FsError::Io(_) | FsError::Watch(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        StatusError::from_code(status)
            .unwrap_or_else(StatusError::internal_server_error)
            .brief(error.to_string())
    }
}
