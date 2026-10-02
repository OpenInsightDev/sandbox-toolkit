# Termination

来源：<https://tus.io/protocols/resumable-upload#termination>

This extension defines a way for the Client to terminate completed and unfinished uploads allowing the Server to free up used resources.

If this extension is supported by the Server, it MUST be announced by adding `termination` to the `Tus-Extension` header.

## Requests

### DELETE

When receiving a `DELETE` request for an existing upload the Server SHOULD free associated resources and MUST respond with the `204 No Content` status confirming that the upload was terminated. For all future requests to this URL, the Server SHOULD respond with the `404 Not Found` or `410 Gone` status.

## Example

**Request:**

```http
DELETE /files/24e533e02ec3bc40c387f1a0e460e216 HTTP/1.1
Host: tus.example.org
Content-Length: 0
Tus-Resumable: 1.0.0
```

**Response:**

```http
HTTP/1.1 204 No Content
Tus-Resumable: 1.0.0
```
