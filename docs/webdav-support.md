# WebDAV RFC Support

## Compliance Level

- RFC 4918 Class 1: supported
- RFC 4918 Class 2: supported
- `OPTIONS` response: `DAV: 1, 2`

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
| `PATCH`     | Partial range overwrite, append, and zero-fill                       |
| `DELETE`    | Delete files and recursively delete directories                      |

## Implemented Features

- `If` header state tokens, `Not`, resource tags, and lock condition evaluation
- Ancestor lock enforcement for `Depth: infinity`
- Lock properties including `lockdiscovery` and `supportedlock`
- Live properties: `creationdate`, `getcontentlength`, `getcontenttype`, `getetag`, `getlastmodified`, `resourcetype`
- Set, read, remove, and COPY/MOVE migration of dead properties
- Request-path percent decoding, root-boundary checks, and traversal protection
- `PATCH` implements single-range partial updates and append operations using `X-Update-Range`
- Other unsupported methods return `501 Not Implemented`

## Known Limitations

- Dead properties and locks are stored in memory and lost after restart
- `getetag` is generated from modification time and size; same-second, same-size changes are not detected
- `If` does not support entity-tag conditions (`["etag"]`)
- MOVE does not migrate locks; locks do not follow moved resources
- `PUT`/`GET` do not return `ETag` or `Last-Modified` headers and do not implement `If-Match`/`If-None-Match`
- `REPORT` is not implemented and returns `501`; versioning, CalDAV, and other extensions are not supported

## Verification Record

The repository's litmus report records 102/102 tests passed across the basic, http, copymove, locks, and props suites; 1 warning was issued and 4 lock-condition tests were skipped.
