# handlers/

## http.rs

- `handle_get_head` — serves files and directory listings; supports text line ranges (`Range: lines=`), advertises applicable range units, emits weak metadata-based ETags, honors date-based `If-Range`, and streams full responses. Line-range parsing rejects malformed, overflowed, suffix, and multi-range syntax.
- `handle_put` — body streamed to file via `StreamReader` + `tokio::io::copy` (zero-copy). Creates parent dirs as needed. Checks lock via middleware.
- `handle_patch` — RFC-0002 partial byte updates (`application/partial-update`) supporting inclusive, open-ended, suffix, sparse, and append ranges. Serializes PATCH snapshots and atomic replacements, caps body/resource memory at 64 MiB, validates HTTP-date and ETag preconditions, and emits a versioned weak ETag after changes. Checks lock via middleware.
- `handle_delete` — resolves path, removes file/dir. Checks lock via middleware.
- `handle_options` — returns an empty response with an `Allow` header listing all supported methods and advertises `partial-update` in `DAV`.

## webdav.rs

- `handle_propfind` — parses `Depth` header and body (`allprop`/`propname`/named props). Calls `webdav::fs::collect_entries`, returns `207 Multi-Status` XML via `webdav::xml`.
- `handle_mkcol` — creates collection (directory). Rejects non-directory targets.
- `handle_copy` — copies source to destination. Checks lock on destination via middleware.
- `handle_move` — moves source to destination. Checks lock on both source and destination via middleware.
- `handle_proppatch` — parses XML body for set/remove actions. Stores dead properties in `DeadPropertyStore`. Checks lock via middleware.

## locks.rs

- `handle_lock` — creates exclusive/shared locks or refreshes existing ones (same token presented). Handles lock-null resource creation for non-existent URLs. Returns `Lock-Token` header + activelock XML.
- `handle_unlock` — removes lock by token. Returns `204 No Content`.
