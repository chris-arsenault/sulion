//! Broker and retrieval, served on the encrypted node port.
//!
//! The node and its PTY tools need the secret broker and retrieval, and that
//! traffic carries secrets, so it must ride the same TLS endpoint the control
//! channel uses — one port, one certificate, one pin. These handlers forward
//! to the sibling containers over the internal Docker network, which never
//! leaves the host.

use std::sync::{Arc, OnceLock};

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use futures::TryStreamExt;

use crate::AppState;

/// Largest request body forwarded upstream. Broker and retrieval payloads are
/// JSON; anything past this is not a legitimate request.
const MAX_FORWARD_BYTES: usize = 16 * 1024 * 1024;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/broker", any(broker))
        .route("/broker/*rest", any(broker))
        .route("/retrieval", any(retrieval))
        .route("/retrieval/*rest", any(retrieval))
}

async fn broker(request: Request) -> Response {
    forward("SULION_SECRET_BROKER_URL", "/broker", request).await
}

async fn retrieval(request: Request) -> Response {
    forward("SULION_RETRIEVAL_URL", "/retrieval", request).await
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// Forwards one request to an internal upstream and streams the response back.
///
/// Only the headers the upstream contracts actually use cross the boundary.
async fn forward(upstream_env: &str, prefix: &str, request: Request) -> Response {
    let Some(base) = std::env::var(upstream_env)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        tracing::warn!(
            upstream = upstream_env,
            "gateway upstream is not configured"
        );
        return StatusCode::BAD_GATEWAY.into_response();
    };

    let suffix = request.uri().path().strip_prefix(prefix).unwrap_or("");
    let mut url = format!("{}{}", base.trim_end_matches('/'), suffix);
    if let Some(query) = request.uri().query() {
        url.push('?');
        url.push_str(query);
    }

    let method = request.method().clone();
    let mut forwarded = client().request(method, &url);
    // An allowlist, so a caller cannot smuggle hop-by-hop headers through.
    //
    // The `x-sulion-*` entries look like identity but are not treated as it.
    // Both upstreams read them as fallbacks for query parameters — see
    // `retrieval::context` and `code_intel::api::root`, which do
    // `query.field.or_else(|| header(..))` — so anything reachable by spoofing
    // a header is already reachable by setting the equivalent query parameter,
    // and forwarding them grants nothing extra.
    //
    // That is what makes this safe, and it is a property of the upstreams, not
    // of this list. If either ever authorises on `x-sulion-pty-id` — scoping a
    // secret grant to it, say — this becomes a spoof from anything on the node
    // LAN, and the header must stop being forwarded.
    for name in [
        "authorization",
        "content-type",
        "accept",
        "x-sulion-repo",
        "x-sulion-cwd",
        "x-sulion-pty-id",
        "x-sulion-workspace-id",
        "x-sulion-agent-session-id",
    ] {
        if let Some(value) = request.headers().get(name) {
            forwarded = forwarded.header(name, value);
        }
    }
    let body = match axum::body::to_bytes(request.into_body(), MAX_FORWARD_BYTES).await {
        Ok(body) => body,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };

    let upstream = match forwarded.body(body).send().await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%url, %error, "gateway upstream is unreachable");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };

    let mut response = Response::builder().status(upstream.status());
    if let Some(content_type) = upstream.headers().get(header::CONTENT_TYPE) {
        response = response.header(header::CONTENT_TYPE, content_type);
    }
    response
        .body(Body::from_stream(
            upstream.bytes_stream().map_err(std::io::Error::other),
        ))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}
