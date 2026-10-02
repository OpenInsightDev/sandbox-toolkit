# Creation With Upload

来源：<https://tus.io/protocols/resumable-upload#creation-with-upload>

The Client MAY include parts of the upload in the initial Creation request using the Creation With Upload extension.

If the Server supports this extension, it MUST advertise this by including `creation-with-upload` in the `Tus-Extension` header. Furthermore, this extension depends directly on the [Creation](./creation.md) extension. Therefore, if the Server does not offer the Creation extension, it MUST NOT offer the Creation With Upload extension either.

The Client MAY include either the entirety or a chunk of the upload data in the body of the `POST` request. In this case, similar rules as for the `PATCH` request and response apply. The Client MUST include the `Content-Type: application/offset+octet-stream` header. The Server SHOULD accept as many bytes as possible and MUST include the `Upload-Offset` header in the response and MUST set its value to the offset of the upload after applying the accepted bytes.

If the Client wants to use this extension, the Client SHOULD verify that it is supported by the Server before sending the `POST` request. In addition, the Client SHOULD include the `Expect: 100-continue` header in the request to receive early feedback from the Server on whether it will accept the creation request, before attempting to transfer the first chunk.

## Example

A non-empty `POST` request is used to create a new upload resource. The `Upload-Offset` header in the response indicates how much data has been accepted.

**Request:**

```http
POST /files HTTP/1.1
Host: tus.example.org
Content-Length: 5
Upload-Length: 100
Tus-Resumable: 1.0.0
Content-Type: application/offset+octet-stream

hello
```

**Response:**

```http
HTTP/1.1 201 Created
Location: https://tus.example.org/files/24e533e02ec3bc40c387f1a0e460e216
Tus-Resumable: 1.0.0
Upload-Offset: 5
```
