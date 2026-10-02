use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use axum::http::header::{self, HeaderMap, HeaderName, HeaderValue};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use bytes::{Buf, Bytes};
use futures_core::Stream;
use tokio::io::{AsyncRead, ReadBuf};

use crate::config::Config;
use crate::datastore::{FileInfo, Lock, Metadata, StoreComposer, Upload};
use crate::error::{Error, Result};

const TUS_RESUMABLE: &str = "tus-resumable";
const TUS_VERSION: &str = "tus-version";
const TUS_EXTENSION: &str = "tus-extension";
const TUS_MAX_SIZE: &str = "tus-max-size";
const UPLOAD_OFFSET: &str = "upload-offset";
const UPLOAD_LENGTH: &str = "upload-length";
const UPLOAD_DEFER_LENGTH: &str = "upload-defer-length";
const UPLOAD_METADATA: &str = "upload-metadata";
const UPLOAD_CONCAT: &str = "upload-concat";
const X_HTTP_METHOD_OVERRIDE: &str = "x-http-method-override";
const X_CONTENT_TYPE_OPTIONS: &str = "x-content-type-options";

const VERSION: &str = "1.0.0";
const OFFSET_CONTENT_TYPE: &str = "application/offset+octet-stream";
/// Matches tusd's default `AcquireLockTimeout`.
const ACQUIRE_LOCK_TIMEOUT: Duration = Duration::from_secs(20);

/// tus endpoint handlers, independent of routing.
pub struct Handler {
    config: Config,
    store: StoreComposer,
    extensions: String,
}

impl Handler {
    pub fn new(mut config: Config, store: StoreComposer) -> Result<Self> {
        if store.core().is_none() {
            return Err(Error::internal("the store composer has no core data store"));
        }

        config.normalize();
        let extensions = extension_list(&config, &store);

        Ok(Self {
            config,
            store,
            extensions,
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn store(&self) -> &StoreComposer {
        &self.store
    }

    /// Comma-separated capability set advertised in `Tus-Extension`.
    pub fn extensions(&self) -> &str {
        &self.extensions
    }

    pub async fn options(&self, _request: Request) -> Response {
        let mut resp = Response::new(Body::empty());
        *resp.status_mut() = StatusCode::OK;

        let headers = resp.headers_mut();
        headers.insert(
            HeaderName::from_static(TUS_VERSION),
            HeaderValue::from_static(VERSION),
        );
        headers.insert(
            HeaderName::from_static(TUS_EXTENSION),
            HeaderValue::from_str(&self.extensions).expect("the extension list is a valid header"),
        );
        if self.config.max_size > 0 {
            insert_str(
                headers,
                HeaderName::from_static(TUS_MAX_SIZE),
                self.config.max_size.to_string(),
            )
            .expect("the max size is a valid header value");
        }

        respond(Ok(resp))
    }

    /// Handles the creation endpoint and, when `X-HTTP-Method-Override` is set,
    /// the resource methods clients cannot send natively.
    pub async fn post(&self, request: Request) -> Response {
        let result = match effective_method(&request) {
            Some(Method::POST) => self.post_inner(request).await,
            Some(Method::PATCH) => self.patch_inner(request).await,
            Some(Method::DELETE) => self.delete_inner(request).await,
            _ => Ok(method_not_allowed()),
        };

        respond(result)
    }

    pub async fn head(&self, request: Request) -> Response {
        match self.head_inner(request).await {
            Ok(resp) => respond(Ok(resp)),
            // A HEAD response must not carry a body for errors either.
            Err(err) => {
                let mut resp = err.into_response();
                *resp.body_mut() = Body::empty();
                respond(Ok(resp))
            }
        }
    }

    pub async fn patch(&self, request: Request) -> Response {
        respond(self.patch_inner(request).await)
    }

    pub async fn delete(&self, request: Request) -> Response {
        respond(self.delete_inner(request).await)
    }

    async fn post_inner(&self, request: Request) -> Result<Response> {
        self.check_version(&request)?;

        let headers = request.headers().clone();
        let contains_chunk =
            header_str(&headers, header::CONTENT_TYPE) == Some(OFFSET_CONTENT_TYPE);

        // Only honour `Upload-Concat` if the store can actually concatenate;
        // otherwise tusd silently treats the request as a plain upload.
        let concat_header = match self.store.concater() {
            Some(_) => header_str(&headers, UPLOAD_CONCAT).unwrap_or_default(),
            None => "",
        };

        if !concat_header.is_empty() && self.config.disable_concatenation {
            return Err(Error::concatenation_unsupported());
        }

        let mut partial_ids = Vec::new();
        let mut partial_uploads: Vec<Box<dyn Upload>> = Vec::new();
        let (size, is_partial, is_final);

        match parse_concat(concat_header, &self.config.base_path)? {
            Concat::None => {
                is_partial = false;
                is_final = false;
                size = parse_new_upload_length(&headers)?;
            }
            Concat::Partial => {
                is_partial = true;
                is_final = false;
                size = parse_new_upload_length(&headers)?;
            }
            Concat::Final(ids) => {
                is_partial = false;
                is_final = true;
                if contains_chunk {
                    return Err(Error::modify_final());
                }

                let core = self.core()?;
                let mut total = 0u64;
                for id in &ids {
                    let upload = core.get_upload(id).await?;
                    let info = upload.info().await?;
                    if info.offset != info.size {
                        return Err(Error::upload_not_finished());
                    }
                    total += info.size;
                    partial_uploads.push(upload);
                }
                partial_ids = ids;
                size = total;
            }
        }

        self.validate_max_size(size)?;

        let info = FileInfo {
            size,
            metadata: parse_metadata(header_str(&headers, UPLOAD_METADATA)),
            is_partial,
            is_final,
            partial_uploads: partial_ids,
            ..FileInfo::default()
        };

        let upload = self.core()?.create_upload(info).await?;
        let mut info = upload.info().await?;
        let id = info.id.clone();
        let url = self.file_url(&id);

        let mut resp = Response::new(Body::empty());
        *resp.status_mut() = StatusCode::CREATED;
        insert_str(resp.headers_mut(), header::LOCATION, url)?;

        if is_final {
            let concater = self.store.concater().ok_or_else(Error::not_implemented)?;
            concater
                .concat_uploads(upload.as_ref(), &partial_uploads)
                .await?;
            info.offset = size;
        }

        if contains_chunk {
            let lock = self.lock_upload(&id).await?;
            let result = self
                .write_chunk(&mut resp, upload.as_ref(), info, request)
                .await;
            unlock_upload(lock).await;
            result?;
        } else if size == 0 {
            self.finish_upload_if_complete(upload.as_ref(), info)
                .await?;
        }

        Ok(resp)
    }

    async fn head_inner(&self, request: Request) -> Result<Response> {
        let id = self.upload_id(&request).ok_or_else(Error::not_found)?;
        let lock = self.lock_upload(&id).await?;
        let result = self.head_locked(&id).await;
        unlock_upload(lock).await;
        result
    }

    async fn head_locked(&self, id: &str) -> Result<Response> {
        let upload = self.core()?.get_upload(id).await?;
        let info = upload.info().await?;

        let mut resp = Response::new(Body::empty());
        *resp.status_mut() = StatusCode::OK;

        let headers = resp.headers_mut();
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        insert_str(
            headers,
            HeaderName::from_static(UPLOAD_OFFSET),
            info.offset.to_string(),
        )?;

        if info.is_partial {
            insert_str(headers, HeaderName::from_static(UPLOAD_CONCAT), "partial")?;
        } else if info.is_final {
            let mut value = String::from("final;");
            for (index, partial) in info.partial_uploads.iter().enumerate() {
                if index > 0 {
                    value.push(' ');
                }
                value.push_str(&self.file_url(partial));
            }
            insert_str(headers, HeaderName::from_static(UPLOAD_CONCAT), value)?;
        }

        if !info.metadata.is_empty() {
            insert_str(
                headers,
                HeaderName::from_static(UPLOAD_METADATA),
                serialize_metadata(&info.metadata),
            )?;
        }

        insert_str(
            headers,
            HeaderName::from_static(UPLOAD_LENGTH),
            info.size.to_string(),
        )?;
        insert_str(headers, header::CONTENT_LENGTH, info.size.to_string())?;

        Ok(resp)
    }

    async fn patch_inner(&self, request: Request) -> Result<Response> {
        self.check_version(&request)?;

        if header_str(request.headers(), header::CONTENT_TYPE) != Some(OFFSET_CONTENT_TYPE) {
            return Err(Error::invalid_content_type());
        }

        let offset = header_str(request.headers(), UPLOAD_OFFSET)
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(Error::invalid_offset)?;

        let id = self.upload_id(&request).ok_or_else(Error::not_found)?;
        let lock = self.lock_upload(&id).await?;
        let result = self.patch_locked(request, &id, offset).await;
        unlock_upload(lock).await;
        result
    }

    async fn patch_locked(&self, request: Request, id: &str, offset: u64) -> Result<Response> {
        let upload = self.core()?.get_upload(id).await?;
        let info = upload.info().await?;

        if info.is_final {
            return Err(Error::modify_final());
        }

        if offset != info.offset {
            return Err(Error::mismatch_offset());
        }

        let mut resp = Response::new(Body::empty());
        *resp.status_mut() = StatusCode::NO_CONTENT;

        // Nothing left to write: mirror the current offset without touching the store.
        if info.offset == info.size {
            insert_str(
                resp.headers_mut(),
                HeaderName::from_static(UPLOAD_OFFSET),
                offset.to_string(),
            )?;
            return Ok(resp);
        }

        // Deferred lengths are not an advertised extension, so a late length is unsupported.
        if header_str(request.headers(), UPLOAD_LENGTH).is_some_and(|value| !value.is_empty()) {
            return Err(Error::not_implemented());
        }

        self.write_chunk(&mut resp, upload.as_ref(), info, request)
            .await?;

        Ok(resp)
    }

    async fn delete_inner(&self, request: Request) -> Result<Response> {
        self.check_version(&request)?;

        let Some(terminater) = self
            .store
            .terminater()
            .filter(|_| !self.config.disable_termination)
        else {
            return Err(Error::not_implemented());
        };

        let id = self.upload_id(&request).ok_or_else(Error::not_found)?;
        let lock = self.lock_upload(&id).await?;
        let result = async {
            let upload = self.core()?.get_upload(&id).await?;
            terminater.terminate(upload.as_ref()).await?;

            let mut resp = Response::new(Body::empty());
            *resp.status_mut() = StatusCode::NO_CONTENT;
            Ok(resp)
        }
        .await;
        unlock_upload(lock).await;
        result
    }

    /// Streams the request body into the upload, clamped to the bytes that are
    /// still missing. Body errors and overflows are only surfaced after the
    /// store has written everything it could, so partial progress is kept.
    async fn write_chunk(
        &self,
        resp: &mut Response,
        upload: &dyn Upload,
        mut info: FileInfo,
        request: Request,
    ) -> Result<()> {
        let offset = info.offset;
        let content_length = header_str(request.headers(), header::CONTENT_LENGTH)
            .and_then(|value| value.parse::<u64>().ok());

        if let Some(length) = content_length
            && offset + length > info.size
        {
            return Err(Error::size_exceeded());
        }

        let mut max_size = info.size.saturating_sub(offset);
        if let Some(length) = content_length {
            max_size = max_size.min(length);
        }

        let state = Arc::new(BodyState::default());
        let body = request.into_body();
        let mut reader = LimitedBody::new(body.into_data_stream(), max_size, Arc::clone(&state));

        let written = upload.write_chunk(offset, &mut reader).await?;

        if let Some(error) = state.take_error() {
            return Err(Error::internal(error.to_string()));
        }
        if state.overflowed() {
            return Err(Error::size_exceeded());
        }

        let new_offset = offset + written;
        insert_str(
            resp.headers_mut(),
            HeaderName::from_static(UPLOAD_OFFSET),
            new_offset.to_string(),
        )?;

        info.offset = new_offset;
        self.finish_upload_if_complete(upload, info).await
    }

    async fn finish_upload_if_complete(&self, upload: &dyn Upload, info: FileInfo) -> Result<()> {
        if info.offset == info.size {
            upload.finish().await?;
        }
        Ok(())
    }

    fn check_version(&self, request: &Request) -> Result<()> {
        if header_str(request.headers(), TUS_RESUMABLE) == Some(VERSION) {
            Ok(())
        } else {
            Err(Error::unsupported_version())
        }
    }

    fn validate_max_size(&self, size: u64) -> Result<()> {
        if self.config.max_size > 0 && size > self.config.max_size {
            return Err(Error::max_size_exceeded());
        }
        Ok(())
    }

    fn core(&self) -> Result<&Arc<dyn crate::datastore::DataStore>> {
        self.store
            .core()
            .ok_or_else(|| Error::internal("the store composer has no core data store"))
    }

    /// Absolute-or-relative URL of an upload resource; [`Config::base_path`]
    /// decides which, and the tus spec allows either.
    fn file_url(&self, id: &str) -> String {
        format!("{}/{}", self.config.base_path, id)
    }

    fn upload_id(&self, request: &Request) -> Option<String> {
        let path = request.uri().path();
        let base = self.config.base_path.as_str();

        let rest = if base.is_empty() {
            path
        } else {
            path.strip_prefix(base)?
        };

        let id = rest.trim_matches('/');
        (!id.is_empty()).then(|| id.to_owned())
    }

    async fn lock_upload(&self, id: &str) -> Result<Option<Box<dyn Lock>>> {
        let Some(locker) = self.store.locker() else {
            return Ok(None);
        };

        let lock = locker.new_lock(id)?;
        match tokio::time::timeout(ACQUIRE_LOCK_TIMEOUT, lock.lock(Box::new(|| {}))).await {
            Ok(result) => result?,
            Err(_) => return Err(Error::lock_timeout()),
        }
        Ok(Some(lock))
    }
}

async fn unlock_upload(lock: Option<Box<dyn Lock>>) {
    if let Some(lock) = lock {
        let _ = lock.unlock().await;
    }
}

fn respond(result: Result<Response>) -> Response {
    let mut resp = match result {
        Ok(resp) => resp,
        Err(err) => err.into_response(),
    };

    let headers = resp.headers_mut();
    headers.insert(
        HeaderName::from_static(TUS_RESUMABLE),
        HeaderValue::from_static(VERSION),
    );
    headers.insert(
        HeaderName::from_static(X_CONTENT_TYPE_OPTIONS),
        HeaderValue::from_static("nosniff"),
    );

    if resp.status() == StatusCode::PRECONDITION_FAILED {
        resp.headers_mut().insert(
            HeaderName::from_static(TUS_VERSION),
            HeaderValue::from_static(VERSION),
        );
    }

    resp
}

fn method_not_allowed() -> Response {
    let mut resp = Response::new(Body::empty());
    *resp.status_mut() = StatusCode::METHOD_NOT_ALLOWED;
    resp
}

fn insert_str(headers: &mut HeaderMap, name: HeaderName, value: impl AsRef<str>) -> Result<()> {
    let value =
        HeaderValue::from_str(value.as_ref()).map_err(|err| Error::internal(err.to_string()))?;
    headers.insert(name, value);
    Ok(())
}

fn header_str(headers: &HeaderMap, name: impl header::AsHeaderName) -> Option<&str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// Applies `X-HTTP-Method-Override` to POST requests, mirroring tusd's
/// middleware. Returns `None` when the override names an unknown method.
fn effective_method(request: &Request) -> Option<Method> {
    if request.method() != Method::POST {
        return Some(request.method().clone());
    }

    match request.headers().get(X_HTTP_METHOD_OVERRIDE) {
        Some(value) => value
            .to_str()
            .ok()
            .and_then(|value| Method::from_bytes(value.as_bytes()).ok()),
        None => Some(Method::POST),
    }
}

fn extension_list(config: &Config, store: &StoreComposer) -> String {
    let mut extensions = String::from("creation,creation-with-upload");

    if store.terminater().is_some() && !config.disable_termination {
        extensions.push_str(",termination");
    }

    if store.concater().is_some() && !config.disable_concatenation {
        extensions.push_str(",concatenation");
    }

    extensions
}

/// `Upload-Defer-Length` is not an advertised extension, so requesting it is
/// `501` and any other value is rejected as an invalid length.
fn parse_new_upload_length(headers: &HeaderMap) -> Result<u64> {
    match header_str(headers, UPLOAD_DEFER_LENGTH) {
        Some("1") => Err(Error::not_implemented()),
        Some("") | None => parse_upload_length(header_str(headers, UPLOAD_LENGTH)),
        Some(_) => Err(Error::invalid_upload_length()),
    }
}

fn parse_upload_length(value: Option<&str>) -> Result<u64> {
    value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .ok_or_else(Error::invalid_upload_length)
}

/// Parses the `Upload-Metadata` header into decoded key/value pairs; elements
/// with malformed base64 or extra fields are dropped, as in tusd.
fn parse_metadata(header: Option<&str>) -> Metadata {
    let mut metadata = Metadata::new();
    let Some(header) = header else {
        return metadata;
    };

    for element in header.split(',') {
        let element = element.trim();
        let parts: Vec<&str> = element.split(' ').collect();

        if parts.len() > 2 {
            continue;
        }

        let key = parts[0];
        if key.is_empty() {
            continue;
        }

        let value = match parts.get(1) {
            Some(encoded) => match BASE64.decode(encoded) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(value) => value,
                    Err(_) => continue,
                },
                Err(_) => continue,
            },
            None => String::new(),
        };

        metadata.insert(key.to_owned(), value);
    }

    metadata
}

fn serialize_metadata(metadata: &Metadata) -> String {
    let mut header = String::new();

    for (key, value) in metadata {
        if !header.is_empty() {
            header.push(',');
        }
        header.push_str(key);
        header.push(' ');
        header.push_str(&BASE64.encode(value));
    }

    header
}

enum Concat {
    None,
    Partial,
    Final(Vec<String>),
}

/// Parses `Upload-Concat`, returning the partial upload IDs for a final upload.
fn parse_concat(header: &str, base_path: &str) -> Result<Concat> {
    if header.is_empty() {
        return Ok(Concat::None);
    }

    if header == "partial" {
        return Ok(Concat::Partial);
    }

    if let Some(list) = header.strip_prefix("final;")
        && !list.is_empty()
    {
        let mut ids = Vec::new();
        for value in list.split(' ') {
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            ids.push(extract_id_from_url(value, base_path)?);
        }

        if !ids.is_empty() {
            return Ok(Concat::Final(ids));
        }
    }

    Err(Error::invalid_concat())
}

fn extract_id_from_url(value: &str, base_path: &str) -> Result<String> {
    let (_, id) = value.split_once(base_path).ok_or_else(Error::not_found)?;
    Ok(id.trim_matches('/').to_owned())
}

/// Mirrors tusd's `bodyReader`: read errors and size overflows are recorded
/// out-of-band so the store observes a clean EOF and keeps its partial write.
#[derive(Default)]
struct BodyState {
    overflow: AtomicBool,
    error: StdMutex<Option<io::Error>>,
}

impl BodyState {
    fn overflowed(&self) -> bool {
        self.overflow.load(Ordering::SeqCst)
    }

    fn take_error(&self) -> Option<io::Error> {
        self.error.lock().expect("body state poisoned").take()
    }

    fn set_error(&self, error: io::Error) {
        let mut slot = self.error.lock().expect("body state poisoned");
        if slot.is_none() {
            *slot = Some(error);
        }
    }
}

struct LimitedBody<S> {
    stream: S,
    chunk: Bytes,
    remaining: u64,
    state: Arc<BodyState>,
}

impl<S> LimitedBody<S> {
    fn new(stream: S, remaining: u64, state: Arc<BodyState>) -> Self {
        Self {
            stream,
            chunk: Bytes::new(),
            remaining,
            state,
        }
    }

    fn poll_stream(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<Bytes, axum::Error>>>
    where
        S: Stream<Item = Result<Bytes, axum::Error>> + Unpin,
    {
        Pin::new(&mut self.stream).poll_next(cx)
    }
}

impl<S> AsyncRead for LimitedBody<S>
where
    S: Stream<Item = Result<Bytes, axum::Error>> + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();

        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }

        loop {
            if this
                .state
                .error
                .lock()
                .expect("body state poisoned")
                .is_some()
            {
                return Poll::Ready(Ok(()));
            }

            if this.remaining == 0 {
                // The upload is full; any further byte is an overflow.
                if !this.chunk.is_empty() {
                    this.state.overflow.store(true, Ordering::SeqCst);
                    return Poll::Ready(Ok(()));
                }

                return match this.poll_stream(cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(None) => Poll::Ready(Ok(())),
                    Poll::Ready(Some(Err(err))) => {
                        this.state.set_error(io::Error::other(err.to_string()));
                        Poll::Ready(Ok(()))
                    }
                    Poll::Ready(Some(Ok(bytes))) if bytes.is_empty() => continue,
                    Poll::Ready(Some(Ok(_))) => {
                        this.state.overflow.store(true, Ordering::SeqCst);
                        Poll::Ready(Ok(()))
                    }
                };
            }

            if this.chunk.is_empty() {
                match this.poll_stream(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(None) => return Poll::Ready(Ok(())),
                    Poll::Ready(Some(Err(err))) => {
                        this.state.set_error(io::Error::other(err.to_string()));
                        return Poll::Ready(Ok(()));
                    }
                    Poll::Ready(Some(Ok(bytes))) => {
                        if bytes.is_empty() {
                            continue;
                        }
                        this.chunk = bytes;
                    }
                }
            }

            let take = this
                .chunk
                .len()
                .min(buf.remaining())
                .min(this.remaining as usize);
            buf.put_slice(&this.chunk[..take]);
            this.chunk.advance(take);
            this.remaining -= take as u64;
            return Poll::Ready(Ok(()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_metadata_and_skips_invalid_elements() {
        let metadata = parse_metadata(Some("foo aGVsbG8=, bar d29ybGQ=, hah INVALID, empty"));
        assert_eq!(metadata.get("foo").map(String::as_str), Some("hello"));
        assert_eq!(metadata.get("bar").map(String::as_str), Some("world"));
        assert!(!metadata.contains_key("hah"));
        assert_eq!(metadata.get("empty").map(String::as_str), Some(""));
    }

    #[test]
    fn serializes_metadata_with_base64_values() {
        let mut metadata = Metadata::new();
        metadata.insert("name".to_owned(), "lunrjs.png".to_owned());
        metadata.insert("empty".to_owned(), String::new());
        assert_eq!(
            serialize_metadata(&metadata),
            "empty ,name bHVucmpzLnBuZw=="
        );
    }

    #[test]
    fn parses_concat_headers() {
        assert!(matches!(parse_concat("", "/files"), Ok(Concat::None)));
        assert!(matches!(
            parse_concat("partial", "/files"),
            Ok(Concat::Partial)
        ));

        match parse_concat("final; /files/a /files/b/", "/files") {
            Ok(Concat::Final(ids)) => assert_eq!(ids, vec!["a".to_owned(), "b".to_owned()]),
            _ => panic!("expected a final upload"),
        }

        match parse_concat("final;http://tus.io/files/aaa/123 /files/bbb/123", "/files") {
            Ok(Concat::Final(ids)) => {
                assert_eq!(ids, vec!["aaa/123".to_owned(), "bbb/123".to_owned()])
            }
            _ => panic!("expected a final upload"),
        }

        assert!(parse_concat("final;", "/files").is_err());
        assert!(parse_concat("final; /files/a", "/files").is_ok());
        assert!(parse_concat("weird", "/files").is_err());
    }

    #[test]
    fn parses_new_upload_length_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static(UPLOAD_LENGTH),
            HeaderValue::from_static("300"),
        );
        assert_eq!(parse_new_upload_length(&headers).unwrap(), 300);

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static(UPLOAD_DEFER_LENGTH),
            HeaderValue::from_static("1"),
        );
        assert_eq!(
            parse_new_upload_length(&headers).unwrap_err().code(),
            crate::error::ErrorCode::NotImplemented
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static(UPLOAD_LENGTH),
            HeaderValue::from_static("-5"),
        );
        assert!(parse_new_upload_length(&headers).is_err());

        assert!(parse_new_upload_length(&HeaderMap::new()).is_err());
    }
}
