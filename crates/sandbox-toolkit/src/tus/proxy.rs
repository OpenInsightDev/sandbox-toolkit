use std::io;
use std::path::PathBuf;

use axum::body::Body;
use axum::extract::Request;
use axum::http::header::{self, HeaderName, HeaderValue};
use axum::http::{HeaderMap, Response};
use hyper_util::rt::TokioIo;
use thiserror::Error;
use tokio::net::UnixStream;

use crate::http::origin_parts;

/// The headers RFC 9110 fixes as hop-by-hop, which a proxy replaces rather than
/// relays.
const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// A socket has no name to put in `Host`; the absolute URLs tusd answers with
/// are built from the forwarded headers instead.
const UPSTREAM_HOST: HeaderValue = HeaderValue::from_static("localhost");

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Connect(#[from] io::Error),
    #[error(transparent)]
    Upstream(#[from] hyper::Error),
}

/// The local endpoint the `/tus` mount relays to.
#[derive(Clone)]
pub struct Upstream {
    socket: PathBuf,
}

impl Upstream {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }

    /// Relays `request` and answers with the response, both streamed.
    pub async fn forward(&self, request: Request) -> Result<Response<Body>, Error> {
        let stream = UnixStream::connect(&self.socket).await?;
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
        // The connection has to be driven while the response body is read. Let
        // go of the sender here and it ends by itself, once that body is done.
        tokio::spawn(async move {
            let _ = connection.await;
        });

        let response = sender.send_request(upstream_request(request)).await?;

        Ok(client_response(response))
    }
}

/// The headers tusd is reached with: the hop-by-hop ones dropped, and the origin
/// the client saw described in the ones it builds absolute URLs from.
fn upstream_request(request: Request) -> Request {
    let (mut parts, body) = request.into_parts();
    let (scheme, host) = origin_parts(&parts.uri, &parts.headers);

    parts.headers = relayed(&parts.headers);
    parts.headers.insert(header::HOST, UPSTREAM_HOST);
    parts
        .headers
        .insert(HeaderName::from_static("x-forwarded-proto"), scheme);
    parts
        .headers
        .insert(HeaderName::from_static("x-forwarded-host"), host);

    Request::from_parts(parts, body)
}

fn client_response(response: Response<hyper::body::Incoming>) -> Response<Body> {
    let (mut parts, body) = response.into_parts();
    parts.headers = relayed(&parts.headers);

    Response::from_parts(parts, Body::new(body))
}

fn relayed(headers: &HeaderMap) -> HeaderMap {
    let hop_by_hop = hop_by_hop(headers);
    let mut relayed = HeaderMap::new();
    for (name, value) in headers {
        if !hop_by_hop.contains(name) {
            relayed.append(name.clone(), value.clone());
        }
    }

    relayed
}

/// The misleadingly-named headers `Connection` lists join the ones above.
fn hop_by_hop(headers: &HeaderMap) -> Vec<HeaderName> {
    let mut names: Vec<HeaderName> = HOP_BY_HOP
        .iter()
        .map(|name| HeaderName::from_static(name))
        .collect();

    for value in headers.get_all(header::CONNECTION) {
        let Ok(value) = value.to_str() else {
            continue;
        };
        names.extend(
            value
                .split(',')
                .filter_map(|listed| HeaderName::from_bytes(listed.trim().as_bytes()).ok()),
        );
    }

    names
}
