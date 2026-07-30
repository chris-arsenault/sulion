//! Serving file bytes over HTTP: content-type headers and `Range` support.
//!
//! Deciding what a file *is* — its MIME, whether its text inlines, the
//! truncation cap — lives in [`crate::file_preview`], because the node builds
//! previews too and must not depend on the HTTP layer to do it. What remains
//! here is the part that only makes sense as a response.

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;

use super::routes::{ApiError, ApiResult};
use crate::file_preview::{content_type, mime_or_octet, PreviewError};

/// Routes deserialize the node's preview into this; the type is owned by the
/// domain module that builds it.
pub use crate::file_preview::FileResponse;

impl From<PreviewError> for ApiError {
    fn from(error: PreviewError) -> Self {
        match error {
            PreviewError::NotFound => ApiError::NotFound,
            PreviewError::BadPath(message) => ApiError::BadRequest(message),
            PreviewError::Io(err) => ApiError::Io(err),
        }
    }
}

/// `Range` support over bytes already loaded — the node reads the file and
/// sends them, so serving never touches the control plane's filesystem.
pub fn serve_loaded_bytes(
    norm: String,
    bytes: Vec<u8>,
    headers: &HeaderMap,
) -> ApiResult<Response> {
    let size = bytes.len() as u64;
    let mime = mime_or_octet(content_type(&norm));
    match parse_range(headers.get(header::RANGE), size) {
        Some(Ok((start, end))) => {
            let slice = bytes[start as usize..=end as usize].to_vec();
            let len = end - start + 1;
            Ok(Response::builder()
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_TYPE, mime)
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"))
                .header(header::CONTENT_LENGTH, len.to_string())
                .body(Body::from(slice))
                .expect("partial-content response is valid"))
        }
        Some(Err(())) => Ok(Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{size}"))
            .header(header::ACCEPT_RANGES, "bytes")
            .body(Body::empty())
            .expect("range-not-satisfiable response is valid")),
        None => Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_LENGTH, size.to_string())
            .body(Body::from(bytes))
            .expect("ok response is valid")),
    }
}

/// Parses a single-range `Range: bytes=start-end` header against a known size.
/// `None` means no range was requested; `Some(Err(()))` means the range cannot
/// be satisfied and the caller must answer 416.
fn parse_range(value: Option<&HeaderValue>, size: u64) -> Option<Result<(u64, u64), ()>> {
    let raw = value?.to_str().ok()?;
    let spec = raw.strip_prefix("bytes=")?;
    if spec.contains(',') {
        return Some(Err(()));
    }
    let (start, end) = spec.split_once('-')?;
    if size == 0 {
        return Some(Err(()));
    }
    let last = size - 1;
    let range = match (start.trim(), end.trim()) {
        ("", "") => return Some(Err(())),
        // Suffix range: the final N bytes.
        ("", n) => {
            let n: u64 = n.parse().ok()?;
            if n == 0 {
                return Some(Err(()));
            }
            (size.saturating_sub(n), last)
        }
        (s, "") => {
            let s: u64 = s.parse().ok()?;
            if s > last {
                return Some(Err(()));
            }
            (s, last)
        }
        (s, e) => {
            let s: u64 = s.parse().ok()?;
            let e: u64 = e.parse().ok()?;
            if s > e || s > last {
                return Some(Err(()));
            }
            (s, e.min(last))
        }
    };
    Some(Ok(range))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hv(value: &str) -> HeaderValue {
        HeaderValue::from_str(value).unwrap()
    }

    async fn body_bytes(response: Response) -> Vec<u8> {
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    #[test]
    fn content_type_is_independent_of_binary_sniff() {
        assert_eq!(content_type("a/b/pic.png"), "image/png");
        assert_eq!(content_type("PIC.JPG"), "image/jpeg");
        assert_eq!(content_type("icon.svg"), "image/svg+xml");
        assert_eq!(content_type("doc.pdf"), "application/pdf");
        assert_eq!(content_type("readme.md"), "text/markdown");
        assert_eq!(content_type("main.rs"), "text/plain");
        assert_eq!(content_type("mystery.xyz"), "");
        assert_eq!(content_type("noext"), "");
    }

    #[test]
    fn parse_range_handles_open_suffix_and_invalid_specs() {
        assert_eq!(parse_range(Some(&hv("bytes=0-9")), 100), Some(Ok((0, 9))));
        assert_eq!(parse_range(Some(&hv("bytes=90-")), 100), Some(Ok((90, 99))));
        assert_eq!(parse_range(Some(&hv("bytes=-10")), 100), Some(Ok((90, 99))));
        // Past the end, reversed, and multi-range are all unsatisfiable.
        assert_eq!(parse_range(Some(&hv("bytes=100-")), 100), Some(Err(())));
        assert_eq!(parse_range(Some(&hv("bytes=9-0")), 100), Some(Err(())));
        assert_eq!(parse_range(Some(&hv("bytes=0-1,3-4")), 100), Some(Err(())));
        assert_eq!(parse_range(None, 100), None);
    }

    /// The size cap lives on the node now, which reads the file and refuses
    /// oversized ones before sending; what remains here is turning those bytes
    /// into a full or partial response.
    #[tokio::test]
    async fn serve_loaded_bytes_full_and_range() {
        let payload: Vec<u8> = (0..=9).collect();

        // Full body carries the real content type and length.
        let full =
            serve_loaded_bytes("pic.png".into(), payload.clone(), &HeaderMap::new()).unwrap();
        assert_eq!(full.status(), StatusCode::OK);
        assert_eq!(full.headers()[header::CONTENT_TYPE], "image/png");
        assert_eq!(full.headers()[header::ACCEPT_RANGES], "bytes");
        assert_eq!(body_bytes(full).await, payload);

        // A Range request yields 206 with just the slice.
        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, hv("bytes=2-4"));
        let partial = serve_loaded_bytes("pic.png".into(), payload.clone(), &headers).unwrap();
        assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(partial.headers()[header::CONTENT_RANGE], "bytes 2-4/10");
        assert_eq!(body_bytes(partial).await, vec![2, 3, 4]);

        // A range past the end is refused rather than clamped or panicking.
        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, hv("bytes=20-30"));
        let unsatisfiable = serve_loaded_bytes("pic.png".into(), payload, &headers).unwrap();
        assert_eq!(unsatisfiable.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(unsatisfiable.headers()[header::CONTENT_RANGE], "bytes */10");
    }
}
