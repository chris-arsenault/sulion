//! Deciding what a file looks like to a viewer: its content type, whether its
//! text is inlined, and the cap past which it is reported as truncated.
//!
//! This is filesystem domain logic, not HTTP. It lives outside `api` because
//! the node runtime builds previews too, and a node reaching into the HTTP
//! layer for them inverted the dependency between the two. `api` turns a
//! [`PreviewError`] into a response; the node turns it into a protocol result.
//!
//! Content type is derived from the path extension and is independent of the
//! binary sniff: a PNG is `image/png` whether or not its bytes look binary. The
//! `binary` flag on a preview means only "not inlined as UTF-8 text", which is
//! a separate question from "what is this file".

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::workspace;

/// Largest text file inlined as UTF-8 in a preview. Larger text is reported as
/// `truncated` so the client fetches raw bytes or opens the file in a terminal.
pub const TEXT_PREVIEW_CAP: u64 = 1024 * 1024; // 1 MiB

/// Why a preview could not be produced. Kept to the distinctions a caller acts
/// on: a path the caller got wrong, a file that is not there, and everything
/// else.
#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    BadPath(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

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

pub fn mime_or_octet(mime: &str) -> String {
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

async fn stat_file(abs: &Path) -> Result<u64, PreviewError> {
    match tokio::fs::metadata(abs).await {
        Ok(meta) if meta.is_file() => Ok(meta.len()),
        Ok(_) => Err(PreviewError::NotFound),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(PreviewError::NotFound),
        Err(err) => Err(PreviewError::Io(err)),
    }
}

/// Build a preview for a repo-relative path under `root`. Inlines text up to
/// `TEXT_PREVIEW_CAP`; reports binary media (image, pdf, …) with its real MIME
/// and no content so the client fetches bytes from the raw route.
pub async fn build_preview(root: PathBuf, rel: &str) -> Result<FileResponse, PreviewError> {
    let (abs, norm) = workspace::resolve_in_repo(&root, rel)
        .map_err(|err| PreviewError::BadPath(err.to_string()))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn preview_inlines_text_and_defers_binary_media() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        tokio::fs::write(root.join("notes.md"), "# hi\n")
            .await
            .unwrap();
        tokio::fs::write(root.join("pic.png"), [0x89, b'P', b'N', b'G', 0x0D])
            .await
            .unwrap();
        tokio::fs::write(root.join("vector.svg"), "<svg/>").await.unwrap();

        let md = build_preview(root.clone(), "notes.md").await.unwrap();
        assert_eq!(md.mime, "text/markdown");
        assert!(!md.binary);
        assert_eq!(md.content.as_deref(), Some("# hi\n"));

        // Binary media keeps its real MIME and defers its bytes to the raw route.
        let png = build_preview(root.clone(), "pic.png").await.unwrap();
        assert_eq!(png.mime, "image/png");
        assert!(png.binary);
        assert!(png.content.is_none());

        // SVG is XML the UI renders, so it keeps content.
        let svg = build_preview(root, "vector.svg").await.unwrap();
        assert_eq!(svg.mime, "image/svg+xml");
        assert!(!svg.binary);
        assert_eq!(svg.content.as_deref(), Some("<svg/>"));
    }

    #[tokio::test]
    async fn preview_separates_a_bad_path_from_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();

        let escape = build_preview(root.clone(), "../outside.txt").await;
        assert!(matches!(escape, Err(PreviewError::BadPath(_))));

        let missing = build_preview(root, "nope.txt").await;
        assert!(matches!(missing, Err(PreviewError::NotFound)));
    }
}
