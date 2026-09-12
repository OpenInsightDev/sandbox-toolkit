# webdav/

## mod.rs — Types & parsers

- `DeadPropertyStore` = `HashMap<PathBuf, HashMap<String, String>>`
- `LockStore` = `Arc<RwLock<HashMap<PathBuf, Vec<LockInfo>>>>`
- `LockInfo`: token, scope (shared/exclusive), owner, depth, timeout, created, updated
- `Depth` enum: `Zero`, `One`, `Infinity`
- `PropEntry`, `PropRequest`, `PropPatchOp`, `PropPatchAction`
- Parsers: `parse_depth`, `parse_propfind_request`, `parse_proppatch_request`, `parse_destination`, `parse_timeout`, `parse_lock_token_header`, `parse_overwrite`

## ls.rs — Lock evaluation (RFC 4918 §10.4)

- `IfCondition` enum: `StateToken(String)` | `Not(Box<IfCondition>)`
- `IfList` = `Vec<Vec<IfCondition>>` (AND of OR-lists)
- `parse_if_header(bytes) → IfList`
- `eval_if(if_list, locks) → bool` — evaluates conditions against lock store
- `active_slice(infos) → Iterator<&&LockInfo>` — lazy expired-lock filter
- `walk_locked_ancestors(path, locks) → Vec<(PathBuf, LockInfo)>` — ancestor chain
- `find_and_refresh_ancestor_lock(path, locks) → Option<LockInfo>` — discovers ancestor lock and refreshes timeout
- `check_existing_exclusive(path, locks) → bool` — conflict check

## method.rs — Method type

Type-safe method enum with `TryFrom<&axum::http::Method>`. Supports all HTTP + WebDAV extension methods (PROPFIND, MKCOL, COPY, MOVE, LOCK, UNLOCK, REPORT, PROPPATCH).

## xml.rs — XML generation

- `El` zero-sized struct: all `D:`-prefixed element name constants (`MULTI_STATUS`, `PROP`, `LOCKDISCOVERY`, `ACTIVE_LOCK`, etc.)
- `XmlWriterExt` trait: `.ev(event)` shorthand for `.write_event(event).unwrap()`
- `write_activelock(lock) → String` — generates activelock XML for LOCK response + PROPFIND lockdiscovery
- `multistatus(xml) → (StatusCode, Response)` — wraps content in `D:multistatus` envelope, returns `207`
- `SUPPORTED_PROPS` const: `creationdate`, `getcontentlength`, `getcontenttype`, `getetag`, `getlastmodified`, `lockdiscovery`, `resourcetype`, `supportedlock`

## fs.rs — Filesystem traversal

- `collect_entries(fs_path, request_path, depth) → Vec<PropEntry>` — recursive dir traversal respecting `Depth`
- `normalize_href(path, is_dir) → String` — ensures trailing `/` for directories
- `guess_content_type(path) → Option<&str>` — MIME type detection via `mime_guess`
- HREF encoding uses `NON_ALPHANUMERIC` set keeping `/`, `-`, `_`, `.`, `~`
