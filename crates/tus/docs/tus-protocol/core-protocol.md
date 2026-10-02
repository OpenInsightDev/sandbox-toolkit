# Core Protocol

来源：<https://tus.io/protocols/resumable-upload#core-protocol>

The core protocol describes how to resume an interrupted upload. It assumes that you already have a URL for the upload, usually created via the [Creation](./creation.md) extension.

All Clients and Servers MUST implement the core protocol.

This specification does not describe the structure of URLs, as that is left for the specific implementation to decide. All URLs shown in this document are meant for example purposes only.

In addition, the implementation of authentication and authorization is left for the Server to decide.

## Example

A `HEAD` request is used to determine the offset at which the upload should be continued.

The example below shows the continuation of a 100 byte upload that was interrupted after 70 bytes were transferred.

**Request:**

```http
HEAD /files/24e533e02ec3bc40c387f1a0e460e216 HTTP/1.1
Host: tus.example.org
Tus-Resumable: 1.0.0
```

**Response:**

```http
HTTP/1.1 200 OK
Upload-Offset: 70
Tus-Resumable: 1.0.0
```

Given the offset, the Client uses the `PATCH` method to resume the upload:

**Request:**

```http
PATCH /files/24e533e02ec3bc40c387f1a0e460e216 HTTP/1.1
Host: tus.example.org
Content-Type: application/offset+octet-stream
Content-Length: 30
Upload-Offset: 70
Tus-Resumable: 1.0.0

[remaining 30 bytes]
```

**Response:**

```http
HTTP/1.1 204 No Content
Tus-Resumable: 1.0.0
Upload-Offset: 100
```

## Headers

### Upload-Offset

The `Upload-Offset` request and response header indicates a byte offset within a resource. The value MUST be a non-negative integer.

### Upload-Length

The `Upload-Length` request and response header indicates the size of the entire upload in bytes. The value MUST be a non-negative integer.

### Tus-Version

The `Tus-Version` response header MUST be a comma-separated list of protocol versions supported by the Server. The list MUST be sorted by Server’s preference where the first one is the most preferred one.

### Tus-Resumable

The `Tus-Resumable` header MUST be included in every request and response except for `OPTIONS` requests. The value MUST be the version of the protocol used by the Client or the Server.

If the version specified by the Client is not supported by the Server, it MUST respond with the `412 Precondition Failed` status and MUST include the `Tus-Version` header into the response. In addition, the Server MUST NOT process the request.

### Tus-Extension

The `Tus-Extension` response header MUST be a comma-separated list of the extensions supported by the Server. If no extensions are supported, the `Tus-Extension` header MUST be omitted.

### Tus-Max-Size

The `Tus-Max-Size` response header MUST be a non-negative integer indicating the maximum allowed size of an entire upload in bytes. The Server SHOULD set this header if there is a known hard limit.

### X-HTTP-Method-Override

The `X-HTTP-Method-Override` request header MUST be a string which MUST be interpreted as the request’s method by the Server, if the header is presented. The actual method of the request MUST be ignored. The Client SHOULD use this header if its environment does not support the PATCH or DELETE methods.

## Requests

### HEAD

The Server MUST always include the `Upload-Offset` header in the response for a `HEAD` request, even if the offset is `0`, or the upload is already considered completed. If the size of the upload is known, the Server MUST include the `Upload-Length` header in the response. If the resource is not found, the Server SHOULD return either the `404 Not Found`, `410 Gone` or `403 Forbidden` status without the `Upload-Offset` header.

The Server SHOULD acknowledge successful `HEAD` requests with a `200 OK` or `204 No Content` status.

The Server MUST prevent the client and/or proxies from caching the response by adding the `Cache-Control: no-store` header to the response.

### PATCH

The Server SHOULD accept `PATCH` requests against any upload URL and apply the bytes contained in the message at the given offset specified by the `Upload-Offset` header. All `PATCH` requests MUST use `Content-Type: application/offset+octet-stream`, otherwise the server SHOULD return a `415 Unsupported Media Type` status.

The `Upload-Offset` header’s value MUST be equal to the current offset of the resource. In order to achieve parallel upload the [Concatenation](./concatenation.md) extension MAY be used. If the offsets do not match, the Server MUST respond with the `409 Conflict` status without modifying the upload resource.

The Client SHOULD send all the remaining bytes of an upload in a single `PATCH` request, but MAY also use multiple small requests successively for scenarios where this is desirable. One example for these situations is when the [Checksum](https://tus.io/protocols/resumable-upload#checksum) extension is used.

The Server MUST acknowledge successful `PATCH` requests with the `204 No Content` status. It MUST include the `Upload-Offset` header containing the new offset. The new offset MUST be the sum of the offset before the `PATCH` request and the number of bytes received and processed or stored during the current `PATCH` request.

If the server receives a `PATCH` request against a non-existent resource it SHOULD return a `404 Not Found` status.

Both Client and Server, SHOULD attempt to detect and handle network errors predictably. They MAY do so by checking for read/write socket errors, as well as setting read/write timeouts. A timeout SHOULD be handled by closing the underlying connection.

The Server SHOULD always attempt to store as much of the received data as possible.

### OPTIONS

An `OPTIONS` request MAY be used to gather information about the Server’s current configuration. A successful response indicated by the `204 No Content` or `200 OK` status MUST contain the `Tus-Version` header. It MAY include the `Tus-Extension` and `Tus-Max-Size` headers.

The Client SHOULD NOT include the `Tus-Resumable` header in the request and the Server MUST ignore the header.

#### Example

This example clarifies the response for an `OPTIONS` request. The version used in the response is `1.0.0` while the Server is also capable of handling `0.2.2` and `0.2.1`. Uploads with a total size of up to 1GB are allowed and the extensions for [Creation](./creation.md) and [Expiration](https://tus.io/protocols/resumable-upload#expiration) are enabled.

**Request:**

```http
OPTIONS /files HTTP/1.1
Host: tus.example.org
```

**Response:**

```http
HTTP/1.1 204 No Content
Tus-Resumable: 1.0.0
Tus-Version: 1.0.0,0.2.2,0.2.1
Tus-Max-Size: 1073741824
Tus-Extension: creation,expiration
```
