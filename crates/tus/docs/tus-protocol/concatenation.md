# Concatenation

来源：<https://tus.io/protocols/resumable-upload#concatenation>

This extension can be used to concatenate multiple uploads into a single one enabling Clients to perform parallel uploads and to upload non-contiguous chunks. If the Server supports this extension, it MUST add `concatenation` to the `Tus-Extension` header.

A partial upload represents a chunk of a file. It is constructed by including the `Upload-Concat: partial` header while creating a new upload using the [Creation](./creation.md) extension. Multiple partial uploads are concatenated into a final upload in the specified order. The Server SHOULD NOT process these partial uploads until they are concatenated to form a final upload. The length of the final upload MUST be the sum of the length of all partial uploads.

In order to create a new final upload, the Client MUST add the `Upload-Concat` header to the upload creation request. The value MUST be `final` followed by a semicolon and a space-separated list of the partial upload URLs that need to be concatenated. The partial uploads MUST be concatenated as per the order specified in the list. This concatenation request SHOULD happen after all of the corresponding partial uploads are completed. The Client MUST NOT include the `Upload-Length` header in the final upload creation.

The Client MAY send the concatenation request while the partial uploads are still in progress. This feature MUST be explicitly announced by the Server by adding `concatenation-unfinished` to the `Tus-Extension` header.

When creating a new final upload the partial uploads’ metadata SHALL NOT be transferred to the new final upload. All metadata SHOULD be included in the concatenation request using the `Upload-Metadata` header.

The Server MAY delete partial uploads after concatenation. The Client, however, MAY attempt to use a partial upload multiple times. The same partial upload MAY be present multiple times in the `Upload-Concat` header in one upload creation request or MAY be used in multiple upload creation requests.

The Server MUST respond with the `403 Forbidden` status to `PATCH` requests against a final upload URL and MUST NOT modify the final or its partial uploads.

The response to a HEAD request for a final upload SHOULD NOT contain the `Upload-Offset` header unless the concatenation has been successfully finished. After successful concatenation, the `Upload-Offset` and `Upload-Length` MUST be set and their values MUST be equal. The value of the `Upload-Offset` header before concatenation is not defined for a final upload.

The response to a HEAD request for a partial upload MUST contain the `Upload-Offset` header.

The `Upload-Length` header MUST be included if the length of the final resource can be calculated at the time of the request. Response to `HEAD` request against partial or final upload MUST include the `Upload-Concat` header and its value as received in the upload creation request.

## Headers

### Upload-Concat

The `Upload-Concat` request and response header MUST be set in both partial and final upload creation requests. It indicates whether the upload is either a partial or final upload. If the upload is a partial one, the header value MUST be `partial`. In the case of a final upload, its value MUST be `final` followed by a semicolon and a space-separated list of partial upload URLs that will be concatenated. The partial uploads URLs MAY be absolute or relative and MUST NOT contain spaces as defined in [RFC 3986](https://www.rfc-editor.org/rfc/rfc3986.html).

## Example

In the following example, the `Host` and `Tus-Resumable` headers are omitted for readability although they are required by the specification. In the beginning two partial uploads are created:

```http
POST /files HTTP/1.1
Upload-Concat: partial
Upload-Length: 5

HTTP/1.1 201 Created
Location: https://tus.example.org/files/a
```

```http
POST /files HTTP/1.1
Upload-Concat: partial
Upload-Length: 6

HTTP/1.1 201 Created
Location: https://tus.example.org/files/b
```

You are now able to upload data to the two partial resources using `PATCH` requests:

```http
PATCH /files/a HTTP/1.1
Upload-Offset: 0
Content-Length: 5

hello

HTTP/1.1 204 No Content
```

```http
PATCH /files/b HTTP/1.1
Upload-Offset: 0
Content-Length: 6

 world

HTTP/1.1 204 No Content
```

In the first request, the string `hello` was uploaded while the second file now contains `  world ` with a leading space.

The next step is to create the final upload consisting of the two earlier generated partial uploads. In the following request, no `Upload-Length` header is presented.

```http
POST /files HTTP/1.1
Upload-Concat: final;/files/a /files/b

HTTP/1.1 201 Created
Location: https://tus.example.org/files/ab
```

The length of the final resource is now 11 bytes consisting of the string `hello world`.

```http
HEAD /files/ab HTTP/1.1

HTTP/1.1 200 OK
Upload-Length: 11
Upload-Concat: final;/files/a /files/b
```
