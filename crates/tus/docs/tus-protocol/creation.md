# Creation

来源：<https://tus.io/protocols/resumable-upload#creation>

Clients and Servers are encouraged to implement as many of the extensions as possible. Feature detection SHOULD be achieved by the Client sending an `OPTIONS` request and the Server responding with the `Tus-Extension` header.

The Client and the Server SHOULD implement the upload creation extension. If the Server supports this extension, it MUST add `creation` to the `Tus-Extension` header.

## Example

An empty `POST` request is used to create a new upload resource. The `Upload-Length` header indicates the size of the entire upload in bytes.

**Request:**

```http
POST /files HTTP/1.1
Host: tus.example.org
Content-Length: 0
Upload-Length: 100
Tus-Resumable: 1.0.0
Upload-Metadata: filename d29ybGRfZG9taW5hdGlvbl9wbGFuLnBkZg==,is_confidential
```

**Response:**

```http
HTTP/1.1 201 Created
Location: https://tus.example.org/files/24e533e02ec3bc40c387f1a0e460e216
Tus-Resumable: 1.0.0
```

The new resource has an implicit offset of `0` allowing the Client to use the core protocol for performing the actual upload.

## Headers

### Upload-Defer-Length

The `Upload-Defer-Length` request and response header indicates that the size of the upload is not known currently and will be transferred later. Its value MUST be `1`. If the length of an upload is not deferred, this header MUST be omitted.

### Upload-Metadata

The `Upload-Metadata` request and response header MUST consist of one or more comma-separated key-value pairs. The key and value MUST be separated by a space. The key MUST NOT contain spaces and commas and MUST NOT be empty. The key SHOULD be ASCII encoded and the value MUST be Base64 encoded. All keys MUST be unique. The value MAY be empty. In these cases, the space, which would normally separate the key and the value, MAY be left out.

Since metadata can contain arbitrary binary values, Servers SHOULD carefully validate metadata values or sanitize them before using them as header values to avoid header smuggling.

## Requests

### POST

The Client MUST send a `POST` request against a known upload creation URL to request a new upload resource. The request MUST include one of the following headers:

a) `Upload-Length` to indicate the size of an entire upload in bytes.

b) `Upload-Defer-Length: 1` if upload size is not known at the time. If the `Upload-Defer-Length` header contains any other value than `1` the server should return a `400 Bad Request` status.

If the length was deferred using `Upload-Defer-Length: 1`, the Client MUST set the `Upload-Length` header in the next `PATCH` request, once the length is known. Once set the length MUST NOT be changed. As long as the length of the upload is not known, the Server MUST set `Upload-Defer-Length: 1` in all responses to `HEAD` requests.

If the Server supports deferring length, it MUST add `creation-defer-length` to the `Tus-Extension` header.

The `Upload-Length` header MAY be set to 0, indicating that the Client wants to upload an empty file. Such an upload is immediately complete after its creation without transferring data using `PATCH` requests.

The Client MAY supply the `Upload-Metadata` header to add additional metadata to the upload creation request. The Server MAY decide to ignore or use this information to further process the request or to reject it. If an upload contains additional metadata, responses to `HEAD` requests MUST include the `Upload-Metadata` header and its value as specified by the Client during the creation.

If the length of the upload exceeds the maximum, which MAY be specified using the `Tus-Max-Size` header, the Server MUST respond with the `413 Request Entity Too Large` status.

The Server MUST acknowledge a successful upload creation with the `201 Created` status. The Server MUST set the `Location` header to the URL of the created resource. This URL MAY be absolute or relative.

The Client MUST perform the actual upload using the core protocol.
