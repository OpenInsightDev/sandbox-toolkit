# handlers/

## http.rs

- `handle_get_head` — serves files (tokio::fs), generates HTML dir listing via `html::generate_dir_listing`. Supports `If-Modified-Since` and `Range` requests. Body streamed via `ReaderStream` / `tokio_util::io::StreamReader`.
- `handle_put` — body streamed to file via `StreamReader` + `tokio::io::copy` (zero-copy). Creates parent dirs as needed. Checks lock via middleware.
- `handle_delete` — resolves path, removes file/dir. Checks lock via middleware.
- `handle_options` — returns `Allow` header listing all supported methods.

## webdav.rs

- `handle_propfind` — parses `Depth` header and body (`allprop`/`propname`/named props). Calls `webdav::fs::collect_entries`, returns `207 Multi-Status` XML via `webdav::xml`.
- `handle_mkcol` — creates collection (directory). Rejects non-directory targets.
- `handle_copy` — copies source to destination. Checks lock on destination via middleware.
- `handle_move` — moves source to destination. Checks lock on both source and destination via middleware.
- `handle_proppatch` — parses XML body for set/remove actions. Stores dead properties in `DeadPropertyStore`. Checks lock via middleware.

## locks.rs

- `handle_lock` — creates exclusive/shared locks or refreshes existing ones (same token presented). Handles lock-null resource creation for non-existent URLs. Returns `Lock-Token` header + activelock XML.
- `handle_unlock` — removes lock by token. Returns `204 No Content`.
