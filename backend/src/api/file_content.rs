//! Shared file preview and raw-byte serving for the repo and workspace file
//! routes. One place decides content types, the text-inline cap, and how bytes
//! are served (with `Range` support); the route handlers stay thin and only
//! differ in how they resolve the repo root and which auth layer guards them.
//!
//! Content type is derived from the path extension and is independent of the
//! binary sniff: a PNG is `image/png` whether or not its bytes look binary. The
//! `binary` flag on a preview means only "not inlined as UTF-8 text", which is
//! a separate question from "what is this file".

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use serde::{Deserialize, Serialize};

use super::routes::{ApiError, ApiResult};
use crate::workspace;

/// Largest text file inlined as UTF-8 in a preview. Larger text is reported as
/// `truncated` so the client fetches raw bytes or opens the file in a terminal.
pub const TEXT_PREVIEW_CAP: u64 = 1024 * 1024; // 1 MiB
/// Largest file served over the authenticated raw-byte route. Bounds memory
/// and keeps a single request from monopolising the proxy; viewing-oriented,
/// generous enough for screenshots and short clips.
pub const RAW_MAX_BYTES: u64 = 25 * 1024 * 1024; // 25 MiB

/// Metadata for a file preview. `content` carries inlined UTF-8 text; it is
/// `None` for binary files, oversized text, and any file whose bytes should be
/// fetched from the raw-byte route instead.
#[derive(Serialize, Deserialize)]
pub struct FileResponse {
    pub path: String,
    pub size: u64,
    pub mime: String,
    pub binary: bool,
    pub truncated: bool,
    pub content: Option<String>,
}

/// True MIME type by extension, independent of the binary sniff. Empty string
/// means "unknown"; callers fall back to `application/octet-stream` for bytes
/// or a text sniff for previews. Deliberately small — the set the UI renders.
pub fn content_type(path: &str) -> &'static str {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "md" | "markdown" => "text/markdown",
        "json" => "application/json",
        "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "go" | "java" | "c" | "cpp" | "h" | "sh"
        | "toml" | "yaml" | "yml" | "html" | "css" | "scss" | "sql" | "txt" => "text/plain",
        _ => "",
    }
}

fn mime_or_octet(mime: &str) -> String {
    if mime.is_empty() {
        "application/octet-stream".into()
    } else {
        mime.into()
    }
}

/// Whether a declared MIME should be inlined as text in a preview. Unknown
/// types fall through to a byte sniff; SVG is XML the UI renders inline.
fn inline_as_text(mime: &str) -> bool {
    mime.is_empty()
        || mime.starts_with("text/")
        || mime == "application/json"
        || mime == "image/svg+xml"
}

async fn stat_file(abs: &Path) -> ApiResult<u64> {
    match tokio::fs::metadata(abs).await {
        Ok(meta) if meta.is_file() => Ok(meta.len()),
        Ok(_) => Err(ApiError::NotFound),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(ApiError::NotFound),
        Err(err) => Err(ApiError::Io(err)),
    }
}

/// Build a preview for a repo-relative path under `root`. Inlines text up to
/// `TEXT_PREVIEW_CAP`; reports binary media (image, pdf, …) with its real MIME
/// and no content so the client fetches bytes from the raw route.
pub async fn build_preview(root: PathBuf, rel: &str) -> ApiResult<FileResponse> {
    let (abs, norm) =
        workspace::resolve_in_repo(&root, rel).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let size = stat_file(&abs).await?;
    let declared = content_type(&norm);

    if !inline_as_text(declared) {
        // Known binary media: metadata only; bytes come from the raw route.
        return Ok(FileResponse {
            path: norm,
            size,
            mime: mime_or_octet(declared),
            binary: true,
            truncated: false,
            content: None,
        });
    }
    if size > TEXT_PREVIEW_CAP {
        return Ok(FileResponse {
            path: norm,
            size,
            mime: if declared.is_empty() {
                "text/plain".into()
            } else {
                declared.into()
            },
            binary: false,
            truncated: true,
            content: None,
        });
    }

    let bytes = tokio::fs::read(&abs).await?;
    let binary = workspace::looks_binary(&bytes);
    let mime = if !declared.is_empty() {
        declared.to_string()
    } else if binary {
        "application/octet-stream".into()
    } else {
        "text/plain".into()
    };
    let content = if binary {
        None
    } else {
        Some(String::from_utf8_lossy(&bytes).into_owned())
    };
    Ok(FileResponse {
        path: norm,
        size,
        mime,
        binary,
        truncated: false,
        content,
    })
}

/// Serve a repo-relative file's bytes with the correct content type and
/// `Range` support. Bounded by `max`; larger files are refused rather than
/// buffered. Used by both the Cognito browser route and (via a thin wrapper)
/// callers that pass their own cap.
pub async fn serve_bytes(
    root: PathBuf,
    rel: &str,
    headers: &HeaderMap,
    max: u64,
) -> ApiResult<Response> {
    let (abs, norm) =
        workspace::resolve_in_repo(&root, rel).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let size = stat_file(&abs).await?;
    if size > max {
        return Err(ApiError::BadRequest(format!(
            "file too large to serve ({size} bytes; limit {max})"
        )));
    }
    let bytes = tokio::fs::read(&abs).await?;
    serve_loaded_bytes(norm, bytes, headers)
}

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
            .expect("static 416 response is valid")),
        None => Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_LENGTH, size.to_string())
            .body(Body::from(bytes))
            .expect("full-body response is valid")),
    }
}

/// Parse a single-range `Range: bytes=…` header against a known size.
/// Returns `None` to serve the whole body (absent or unparsable — per the
/// spec, an unsatisfiable *syntax* is ignored), `Some(Ok((start, end)))` for a
/// satisfiable inclusive range, or `Some(Err(()))` for a valid-but-unsatisfiable
/// range (→ 416). `end` is clamped to the last byte.
fn parse_range(header: Option<&HeaderValue>, size: u64) -> Option<Result<(u64, u64), ()>> {
    let raw = header?.to_str().ok()?;
    let spec = raw.strip_prefix("bytes=")?;
    // Only single ranges are supported; a comma means multipart, which we skip.
    if spec.contains(',') {
        return None;
    }
    let (start_s, end_s) = spec.split_once('-')?;
    let start_s = start_s.trim();
    let end_s = end_s.trim();

    if start_s.is_empty() {
        // Suffix range: last N bytes.
        let suffix: u64 = end_s.parse().ok()?;
        if suffix == 0 || size == 0 {
            return Some(Err(()));
        }
        let start = size.saturating_sub(suffix);
        return Some(Ok((start, size - 1)));
    }

    let start: u64 = start_s.parse().ok()?;
    if start >= size {
        return Some(Err(()));
    }
    let end = if end_s.is_empty() {
        size - 1
    } else {
        let requested: u64 = end_s.parse().ok()?;
        requested.min(size - 1)
    };
    if end < start {
        return Some(Err(()));
    }
    Some(Ok((start, end)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

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

    fn hv(s: &str) -> HeaderValue {
        HeaderValue::from_str(s).unwrap()
    }

    #[test]
    fn parse_range_full_and_open_ended() {
        assert_eq!(parse_range(None, 100), None);
        assert_eq!(parse_range(Some(&hv("bytes=0-9")), 100), Some(Ok((0, 9))));
        assert_eq!(parse_range(Some(&hv("bytes=10-")), 100), Some(Ok((10, 99))));
        // End past EOF clamps to the last byte.
        assert_eq!(
            parse_range(Some(&hv("bytes=90-500")), 100),
            Some(Ok((90, 99)))
        );
    }

    #[test]
    fn parse_range_suffix() {
        assert_eq!(parse_range(Some(&hv("bytes=-20")), 100), Some(Ok((80, 99))));
        // Suffix larger than the file starts at 0.
        assert_eq!(parse_range(Some(&hv("bytes=-500")), 100), Some(Ok((0, 99))));
    }

    #[test]
    fn parse_range_unsatisfiable_and_malformed() {
        assert_eq!(parse_range(Some(&hv("bytes=100-200")), 100), Some(Err(())));
        assert_eq!(parse_range(Some(&hv("bytes=-0")), 100), Some(Err(())));
        // Malformed syntax is ignored → serve the whole body.
        assert_eq!(parse_range(Some(&hv("items=0-9")), 100), None);
        assert_eq!(parse_range(Some(&hv("bytes=abc")), 100), None);
        assert_eq!(parse_range(Some(&hv("bytes=0-9,20-29")), 100), None);
    }

    async fn body_bytes(resp: Response) -> Vec<u8> {
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    #[tokio::test]
    async fn preview_inlines_text_and_defers_binary_media() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        tokio::fs::write(root.join("notes.md"), b"# hi\nsee main.rs:1")
            .await
            .unwrap();
        tokio::fs::write(root.join("logo.png"), [0x89, b'P', b'N', b'G', 0, 1, 2])
            .await
            .unwrap();
        tokio::fs::write(root.join("vector.svg"), b"<svg/>")
            .await
            .unwrap();

        let text = build_preview(root.clone(), "notes.md").await.unwrap();
        assert_eq!(text.mime, "text/markdown");
        assert!(!text.binary && !text.truncated);
        assert_eq!(text.content.as_deref(), Some("# hi\nsee main.rs:1"));

        // A PNG keeps its real type but defers the bytes to the raw route.
        let png = build_preview(root.clone(), "logo.png").await.unwrap();
        assert_eq!(png.mime, "image/png");
        assert!(png.binary && !png.truncated);
        assert!(png.content.is_none());

        // SVG is XML the UI renders inline, so it keeps content.
        let svg = build_preview(root, "vector.svg").await.unwrap();
        assert_eq!(svg.mime, "image/svg+xml");
        assert!(!svg.binary);
        assert_eq!(svg.content.as_deref(), Some("<svg/>"));
    }

    #[tokio::test]
    async fn serve_bytes_full_range_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let payload: Vec<u8> = (0..=9).collect();
        tokio::fs::write(root.join("pic.png"), &payload)
            .await
            .unwrap();

        // Full body carries the real content type and length.
        let full = serve_bytes(root.clone(), "pic.png", &HeaderMap::new(), RAW_MAX_BYTES)
            .await
            .unwrap();
        assert_eq!(full.status(), StatusCode::OK);
        assert_eq!(full.headers()[header::CONTENT_TYPE], "image/png");
        assert_eq!(full.headers()[header::ACCEPT_RANGES], "bytes");
        assert_eq!(body_bytes(full).await, payload);

        // A Range request yields 206 with just the slice.
        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, hv("bytes=2-4"));
        let partial = serve_bytes(root.clone(), "pic.png", &headers, RAW_MAX_BYTES)
            .await
            .unwrap();
        assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(partial.headers()[header::CONTENT_RANGE], "bytes 2-4/10");
        assert_eq!(body_bytes(partial).await, vec![2, 3, 4]);

        // Over the cap is refused rather than buffered.
        let err = serve_bytes(root, "pic.png", &HeaderMap::new(), 4)
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
    }
}
