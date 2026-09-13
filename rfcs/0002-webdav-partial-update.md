# RFC-0002: WebDAV Partial Byte Updates

| Field | Value |
| --- | --- |
| Number | RFC-0002 |
| Title | WebDAV Partial Byte Updates |
| Status | Complete |
| Authors | sandbox-toolkit contributors |
| Created | 2026-09-13 |
| Updated | 2026-09-13 |

## Abstract

Define a neutral HTTP `PATCH` profile for updating a byte interval of an existing WebDAV resource or appending bytes to it. The request carries the replacement bytes and identifies the update operation with `X-Update-Range`. This RFC does not depend on a particular server, library, programming language, or storage backend.

The design is inspired by the [HTTP PATCH support documentation](https://sabre.io/dav/http-patch/), while replacing implementation-specific names with protocol-neutral names.

## Motivation, goals, and non-goals

A client often needs to replace or append a small portion of a large file. A full `PUT` requires transferring and rewriting the complete representation, while a partial update can reduce bandwidth and write amplification.

Goals:

- replace one inclusive byte interval without downloading the complete resource;
- append bytes atomically to the current end of a resource;
- make the request body length and update interval unambiguous;
- preserve ordinary HTTP authentication, authorization, conditional requests, locking, and error handling;
- provide a discoverable WebDAV capability.

Non-goals:

- multiple ranges in one request;
- line, character, or structured-document updates;
- server-side transcoding or patch formats such as JSON Patch;
- defining a new storage API or requiring a particular implementation interface;
- replacing the standard HTTP `PATCH` semantics of RFC 5789 for other media types.

## Terminology and conventions

- A **byte offset** is zero-based.
- A **byte interval** is inclusive at both ends. `bytes=3-6` selects bytes 3, 4, 5, and 6.
- **Partial-update media type** means `application/partial-update` as defined by this RFC. It identifies this request profile; the payload itself is the sequence of replacement bytes.
- `X-Update-Range` is a request header whose value is either a byte interval or the literal `append`. Header field names are case-insensitive; the range-unit token is case-insensitive, while `append` is matched case-insensitively.
- A request body is an opaque byte sequence. It MUST NOT be decoded as text.

## Protocol design

### Discovery

A WebDAV server supporting this profile MUST advertise the `partial-update` capability in the `DAV` response header to `OPTIONS`:

```http
HTTP/1.1 204 No Content
DAV: 1, 2, 3, partial-update
Allow: OPTIONS, GET, HEAD, PUT, PATCH, DELETE, PROPFIND
```

The token is a capability name, not a product or library name. Non-WebDAV HTTP services MAY implement the profile without adding it to `DAV`.

### Request requirements

A partial update request has this form:

```http
PATCH /files/example.bin HTTP/1.1
Host: dav.example.test
Content-Type: application/partial-update
Content-Length: 4
X-Update-Range: bytes=3-6
If-Match: "resource-7b2c"

ABCD
```

The server MUST require `Content-Type`, `Content-Length`, and `X-Update-Range` for this profile. The `Content-Type` MAY include media-type parameters, which the server MUST ignore unless separately defined. `Content-Length` MUST equal the number of bytes received in the request body; a server MUST reject a conflicting transfer framing or truncated body.

For `bytes=<start>-<end>`, both offsets are non-negative decimal integers, `start <= end`, and the body length MUST equal `end - start + 1`. The end offset MAY be outside the current representation. The start offset MAY be outside the current representation.

For `bytes=<start>-`, `start` is non-negative. The request body MUST be non-empty, and the update extends from `start` through `start + Content-Length - 1`. Both the addition and the inclusive end calculation MUST be checked for numeric overflow; an overflow or zero-length body is invalid and MUST be rejected with `416 Range Not Satisfiable`.

For `bytes=-<length>`, `length` is a positive decimal integer. The update targets the final `length` bytes of the current representation, and the body length MUST equal `length`. If `length` exceeds the current size, the update starts at offset zero and the body length MUST equal the resulting selected interval; implementations MUST reject an inconsistent request rather than silently truncate it.

For `append`, the body is written immediately after the current final byte. An empty body is valid and leaves the representation unchanged, subject to normal conditional and authorization checks.

The server MUST reject multiple ranges, whitespace-separated forms, a missing start offset, negative values in a two-sided interval, numeric overflow, and unknown `X-Update-Range` values. A server MUST NOT interpret an ordinary `PATCH` with another content type as this profile.

### Update semantics

The update MUST be applied to one consistent representation snapshot. For an interval, bytes in the selected interval are replaced by the request body. Bytes outside the interval are preserved. If the first updated offset is greater than the current representation length, the gap MUST be filled with `0x00` bytes. If the end offset exceeds the current length, the representation grows to include the complete replacement interval.

For example, applying `bytes=12-` with body `----` to `1234567890` produces ten original bytes, two `0x00` bytes, and four dashes. Applying `append` with the same body produces `1234567890----`.

The operation SHOULD be atomic: readers MUST observe either the old representation or the complete new representation, never a partially written interval. A successful update MUST produce a new representation validator when the underlying resource supports validators. A validator derived only from coarse metadata such as second-resolution modification time and size MUST be marked weak (`W/`) and MUST NOT be accepted as a strong validator for `If-Match` or `If-Range`; implementations that need strong preconditions MUST use a collision-resistant representation validator.

### Responses and status codes

On success, the server MUST return `204 No Content` and no response body. It MAY return `200 OK` with a representation if that behavior is explicitly documented. A `204` response MUST NOT include resource bytes.

The server MUST use these statuses:

| Status | Condition |
| --- | --- |
| `200 OK` or `204 No Content` | Update succeeded; `204` is the default defined by this RFC. |
| `400 Bad Request` | `X-Update-Range` syntax or semantics are invalid. |
| `404 Not Found` | The target does not exist and the server does not support creating it through this profile. |
| `411 Length Required` | `Content-Length` is absent and no equivalent accepted framing is available. |
| `412 Precondition Failed` | An `If-Match`, `If-Unmodified-Since`, or other applicable precondition fails. |
| `413 Content Too Large` | A configured request or resource limit is exceeded. |
| `415 Unsupported Media Type` | `Content-Type` is not `application/partial-update`. |
| `416 Range Not Satisfiable` | The interval/body length relationship is invalid or the requested suffix cannot be resolved. |
| `423 Locked` | WebDAV locking policy rejects the update. |

A malformed or rejected request MUST NOT partially modify the resource. Existing authentication, authorization, `405 Method Not Allowed`, and server-error behavior remains unchanged.

### Conditional requests, locking, and caching

Clients SHOULD send `If-Match` or another strong precondition when the update is based on a previously retrieved representation. The server MUST evaluate applicable conditional request headers using ordinary HTTP semantics before modifying the resource. A server implementing WebDAV locks MUST enforce the lock token policy for `PATCH` exactly as for other write methods.

A successful response MUST invalidate cached representations of the target according to HTTP caching rules. A `PATCH` response is not itself a cacheable representation of the target. Servers SHOULD return `ETag` and `Last-Modified` for the new representation when available.

## Grammar

The following ABNF applies after HTTP field-value whitespace has been removed according to HTTP field parsing rules:

```abnf
update-range = byte-range / "append"
byte-range  = "bytes=" byte-start "-" [ byte-end ]
             / "bytes=-" suffix-length
byte-start  = 1*DIGIT
byte-end    = 1*DIGIT
suffix-length = 1*DIGIT
```

The grammar permits `0`; semantic validation rejects `bytes=-0`. Decimal values MUST be parsed with overflow detection. A two-sided range MUST contain non-negative values and satisfy `end >= start`.

## Examples

Assume the current representation contains the ten ASCII bytes `1234567890` and the request body contains four ASCII dashes (`----`).

Replacing bytes 3 through 6 (zero-based) produces `123----890`:

```http
PATCH /file.txt HTTP/1.1
Host: dav.example.test
Content-Type: application/partial-update
Content-Length: 4
X-Update-Range: bytes=3-6

----
```

Appending produces `1234567890----`:

```http
PATCH /file.txt HTTP/1.1
Host: dav.example.test
Content-Type: application/partial-update
Content-Length: 4
X-Update-Range: append

----
```

A suffix update replaces the final four bytes and produces `123456----`:

```http
PATCH /file.txt HTTP/1.1
Host: dav.example.test
Content-Type: application/partial-update
Content-Length: 4
X-Update-Range: bytes=-4

----
```

The following request is rejected with `416 Range Not Satisfiable` because the body has four bytes but `bytes=2-8` selects seven bytes:

```http
PATCH /file.txt HTTP/1.1
Host: dav.example.test
Content-Type: application/partial-update
Content-Length: 4
X-Update-Range: bytes=2-8

----
```

## Security, performance, and observability

Partial writes MUST use the same path canonicalization, authorization, authentication, and lock enforcement as `PUT` and other write methods. Servers MUST avoid writing outside the configured resource root and MUST validate all offsets before allocating gap or replacement buffers. Implementations SHOULD cap body size, resulting representation size, and zero-filled gap size, returning `413` consistently when a configured limit is exceeded.

Implementations SHOULD use bounded memory and seekable writes where possible. Suffix ranges may require obtaining the current size before writing. Logs SHOULD record the method, target identifier, update kind, interval, status, and duration, but MUST NOT record request bodies or credentials.

## Implementation and rollout

Implementations SHOULD initially gate this profile behind configuration and enable it only for resources whose storage backend supports atomic or transactional replacement. A server MAY advertise `partial-update` only for collections or resource classes where it is enabled. Rollout should begin with interval replacement and append, followed by suffix and sparse writes if those operations are supported.

No language-level interface is required. An implementation may expose an internal operation equivalent to `replace_range(start, bytes)` and `append(bytes)`, but such details are outside this RFC.

## Test plan and acceptance criteria

The executable suite in `tests/rfc_0002_webdav_partial_update.rs` is normative acceptance coverage for this RFC. Every normative request form, response status, safety property, and corner case below is represented by a named test; changes to this RFC MUST update that suite in the same change.

- Accept exact inclusive intervals, open-ended intervals, suffix intervals, and `append`.
- Verify replacement, append, growth, and `0x00` gap filling against known byte sequences, including empty append.
- Reject missing headers, unsupported media types, malformed ranges, overflow, multiple ranges, whitespace, invalid ordering, zero suffixes, and body-length mismatches with the specified status.
- Verify rejected requests never partially modify the resource, including failures before and during interval validation.
- Verify `404`, `411`, `412`, `415`, `416`, `423`, `413`, authentication, authorization, lock tokens, path protection, HTTP-date ordering, and concurrent-update preconditions. This implementation caps request bodies and resulting resources at 64 MiB; oversized requests/resources and allocation refusal return `413`.
- Verify successful responses are `204` with no body, produce a changed validator for same-size replacements, return the new validator on both PATCH and GET, and invalidate stale cached representations.
- Verify `OPTIONS` advertises `PATCH` and `partial-update` only when enabled, and ordinary `PATCH` behavior remains unchanged for other media types.

Traceability matrix:

| RFC requirement | Executable test |
| --- | --- |
| Discovery and method support | `options_advertises_partial_update_capability` |
| Content-Type/Length/range headers | `missing_headers_and_media_type_fail_with_specified_statuses` |
| Inclusive, open-ended, suffix, append semantics | `interval_append_open_ended_and_suffix_updates` |
| Growth and zero-filled gaps | `out_of_bounds_updates_grow_and_zero_fill` |
| Empty append and opaque bytes | `empty_append_and_binary_payload_are_supported` |
| Syntax, overflow, ordering, and body mismatches | `malformed_and_inconsistent_ranges_are_rejected_without_writes` |
| Missing target, path protection, and auth | `not_found_traversal_and_auth_are_preserved` |
| Preconditions and locks | `if_match_and_lock_tokens_are_enforced` and `if_unmodified_since_uses_http_date_ordering` |
| Size limits | `oversized_patch_is_rejected_before_body_processing` |
| No partial writes and response body | `malformed_and_inconsistent_ranges_are_rejected_without_writes` and `interval_append_open_ended_and_suffix_updates` |
| Validator/cache/concurrency behavior | `successful_update_changes_validator_and_serializes_writes` |

## Open questions

1. Should a future revision register a permanent media type, or should deployments use a profile parameter on `application/octet-stream`?
2. Should creation of a missing resource through `PATCH` be standardized, or remain implementation-specific?
3. Should a future RFC define a response representation describing the changed interval?

## Changelog

- 2026-09-13: Initial draft based on the neutralized partial-update behavior described in the [reference documentation](https://sabre.io/dav/http-patch/).
- 2026-09-13: Marked complete after implementing PATCH dispatch, validation, atomic updates, lock enforcement, and the RFC acceptance suite in `tests/rfc_0002_webdav_partial_update.rs`.

## References

- RFC 5789, PATCH Method for HTTP: <https://www.rfc-editor.org/rfc/rfc5789>
- RFC 9110, HTTP Semantics: <https://www.rfc-editor.org/rfc/rfc9110>
- RFC 4918, WebDAV: <https://www.rfc-editor.org/rfc/rfc4918>
- Reference implementation documentation (source inspiration): <https://sabre.io/dav/http-patch/>
