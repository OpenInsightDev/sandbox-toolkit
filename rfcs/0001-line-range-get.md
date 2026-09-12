# RFC-0001: WebDAV GET Line Ranges

| Field | Value |
| --- | --- |
| Number | RFC-0001 |
| Title | WebDAV GET Line Ranges |
| Status | Draft |
| Authors | sandbox-toolkit contributors |
| Created | 2026-09-12 |
| Updated | 2026-09-12 |

## Abstract

Define the `lines` HTTP Range unit for WebDAV `GET` and `HEAD`. A client can request a contiguous range of lines from a text representation. The extension uses RFC 9110 Range/Content-Range/`206 Partial Content` semantics and does not change `bytes` ranges.

## Motivation and scope

Byte ranges force clients to download, decode, and split more data than needed. Line ranges support efficient reads of logs, configuration, and source files.

Goals: preserve the representation's original bytes and line endings; support one contiguous range; and remain compatible with existing byte ranges, validators, caching, `GET`, and `HEAD`.

Non-goals: multipart ranges, character/column ranges, server-side transcoding, or mutation.

## Terminology and conventions

- A **text representation** has a `text/*` media type or is explicitly configured as text. A server MUST NOT treat a binary resource as text solely because `lines` was requested.
- Lines are numbered from 1; both endpoints are inclusive.
- `LF` (`0x0A`), `CRLF` (`0x0D 0x0A`), and lone `CR` (`0x0D`) delimit lines. `CRLF` is one delimiter.
- A non-empty unterminated tail is a line; an empty file has zero lines. Selected bytes include each line's original delimiter, when present. The server MUST NOT transcode or normalize the representation.

## Protocol

### Discovery

A server supporting this extension MUST advertise it:

```http
Accept-Ranges: bytes, lines
```

It MAY advertise only the range units it supports. Absence of `lines` does not promise support.

### Request

```http
GET /docs/notes.txt HTTP/1.1
Host: dav.example.test
Range: lines=10-19
```

The syntax is one range, `lines=<first>-<last>`, where both values are decimal integers, `first >= 1`, and `last >= first`. An omitted `<last>` (`lines=10-`) means through the final line. Suffix ranges (`lines=-10`), comma-separated ranges, whitespace, and other parameters are not defined.

A request without `Range` is unchanged. `Range: bytes=...` retains RFC 9110 semantics. A request MUST NOT mix `bytes` and `lines`; multiple units are invalid rather than precedence-ordered. Per RFC 9110, `Range` is processed only on `GET`; `HEAD` ignores `Range` and describes the complete current representation with the normal `200` response.

### Successful response

For a valid range on a text representation, the server MUST return `206 Partial Content`:

```http
HTTP/1.1 206 Partial Content
Content-Type: text/plain; charset=utf-8
Accept-Ranges: bytes, lines
Content-Range: lines 10-19/240
Content-Length: 863
ETag: "notes-7b2c"

<the original bytes of lines 10 through 19>
```

`Content-Range` reports the current total line count. For `lines=10-`, its end is the actual final line. If the requested end exceeds that total, the end is clipped to the actual final line. The response body MUST contain only the selected complete lines, in order, with no wrapper, added delimiter, or transcoding. `Content-Length` is bytes, not lines.

Line boundaries are computed on the identity-coded representation. If content negotiation selects a non-identity `Content-Encoding`, the server MUST ignore `Range: lines` and send the complete encoded representation; clients requiring line ranges SHOULD request `Accept-Encoding: identity`.

`HEAD` ignores `Range` as required by RFC 9110 and returns the normal complete-representation status and headers without a body.

### Unsatisfiable requests

- An unknown or unsupported range unit MUST be ignored according to RFC 9110, producing the normal complete response (usually `200`).
- For a non-text representation, malformed `lines` syntax, or mixed units, return `416 Range Not Satisfiable`.
- A range is satisfiable when its first line exists. If `<last>` exceeds the final line, clip it to the final line and return `206` with the actual interval.
- For a range whose first line is beyond the current line count, return `416` with `Content-Range: lines */<total>`.
- For an empty file, return `416` with `Content-Range: lines */0`.
- Preserve existing `404`, authentication, and authorization behavior.

The server MUST NOT silently return the full file to satisfy a line-range request.

### Validators, concurrency, and caching

`If-Range` MUST behave as for byte ranges: a matching validator permits `206`; a non-matching validator causes `Range` to be ignored and the current full representation (normally `200`) to be sent. Existing `If-Match`, `If-None-Match`, `If-Modified-Since`, and `If-Unmodified-Since` semantics are unchanged.

Use validators for the same representation in line-range responses. Follow HTTP cache rules; a line-range response MUST NOT be marked or reused as a complete representation, or reused for a different range.

## Grammar

```abnf
lines-range = "lines=" line-pos "-" [ line-pos ]
line-pos    = %x31-39 *(DIGIT) ; decimal integer >= 1
```

HTTP range-unit names are case-insensitive. Examples use lowercase `lines`. Numeric overflow MUST be rejected.

## Examples

For `notes.txt`:

```text
alpha\n
bravo\r\n
charlie
```

`Range: lines=2-3` returns `206`, body `bravo\r\ncharlie`, and `Content-Range: lines 2-3/3`. `Range: lines=2-999` is clipped and returns `206` with `Content-Range: lines 2-3/3`. `Range: lines=4-5` returns `416` with `Content-Range: lines */3`. `Range: lines=2-` returns lines 2 and 3 with `Content-Range: lines 2-3/3`.

## Security, performance, and observability

Line scanning may require reading from the beginning, especially on non-seekable backends. Implementations SHOULD cap scanned bytes, selected lines, and response size, using existing resource-limit errors when exceeded. Line parsing MUST NOT execute file content. Logs SHOULD record unit, requested range, status, and duration, not file contents. Existing path canonicalization and authorization MUST apply.

## Implementation and rollout

This extension SHOULD initially be disabled or limited to configured text media types. Phase one supports one `lines` range on `GET`, normal `HEAD` handling, `206`/`416`, and `Accept-Ranges`. Requests without this header remain unchanged. Multipart or character ranges require separate RFCs.

## Test plan and acceptance criteria

- Correct counts and original bytes for CRLF, LF, CR, unterminated final lines, and empty files.
- Correct `206`, `Content-Range`, and `Content-Length` for `1-1`, middle, and open-ended ranges.
- Correct errors for first-line-out-of-range, suffix, multiple, mixed-unit, and binary requests; clip an oversized end line.
- `HEAD` ignores `Range`, describes the complete representation, and has no body.
- `If-Range`, resource changes, authentication, and path protection do not regress.
- Unsupported clients retain existing full-GET and byte-range behavior.

## Open questions

1. Should the project publish a default list and configuration format for text media types?
2. Should a future RFC define multipart line ranges?

## Changelog

- 2026-09-12: Initial draft.

## References

- RFC 9110, HTTP Semantics, Range Requests: <https://www.rfc-editor.org/rfc/rfc9110#name-range-requests>
- RFC 4918, WebDAV: <https://www.rfc-editor.org/rfc/rfc4918>
- Rust RFCs: <https://github.com/rust-lang/rfcs>
- Bytecode Alliance RFC process: <https://github.com/bytecodealliance/rfcs>
