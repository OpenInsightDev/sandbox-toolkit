# WebDAV RFC Support

## Compliance Level

- RFC 4918 Class 1: supported
- RFC 4918 Class 2: supported
- RFC-0002 partial updates: supported
- `OPTIONS` response: `DAV: 1, 2, 3, partial-update`

## Supported Methods

| Method      | Supported features                                                   |
| ----------- | -------------------------------------------------------------------- |
| `PROPFIND`  | `Depth: 0/1/infinity`, `allprop`, `propname`, named properties       |
| `PROPPATCH` | Set and remove dead properties                                       |
| `MKCOL`     | Create collections; validate parent directory and request body       |
| `COPY`      | Copy files and directories; supports `Depth: 0` and `Overwrite`      |
| `MOVE`      | Move files and directories; supports `Overwrite`                     |
| `LOCK`      | Exclusive/shared locks, lock-null resources, refresh, timeout, depth |
| `UNLOCK`    | Unlock by `Lock-Token`                                               |
| `PUT`       | Create and overwrite files; streaming writes                         |
| `PATCH`     | RFC-0002 partial range updates, append, sparse zero-fill, and atomic replacement |
| `DELETE`    | Delete files and recursively delete directories                      |

## Implemented Features

- `If` header state tokens, `Not`, resource tags, and lock condition evaluation
- Ancestor lock enforcement for `Depth: infinity`
- Lock properties including `lockdiscovery` and `supportedlock`
- Live properties: `creationdate`, `getcontentlength`, `getcontenttype`, `getetag`, `getlastmodified`, `resourcetype`
- Set, read, remove, and COPY/MOVE migration of dead properties
- Request-path percent decoding, root-boundary checks, and traversal protection
- RFC-0002 PATCH requests using `Content-Type: application/partial-update`, `Content-Length`, and `X-Update-Range`
- PATCH conditional requests (`If-Match`, `If-Unmodified-Since`) and lock enforcement
- Weak versioned `ETag` and `Last-Modified` validators for GET, HEAD, and successful PATCH responses
- Other unsupported methods return `501 Not Implemented`

## Known Limitations

- Dead properties and locks are stored in memory and lost after restart
- MOVE does not migrate locks; locks do not follow moved resources
- `REPORT` is not implemented and returns `501`; versioning, CalDAV, and other extensions are not supported

## Verification Record

The repository's litmus report records 102/102 tests passed across the basic, http, copymove, locks, and props suites; 1 warning was issued and 4 lock-condition tests were skipped. RFC-0002 acceptance tests are in `tests/rfc_0002_webdav_partial_update.rs`.
