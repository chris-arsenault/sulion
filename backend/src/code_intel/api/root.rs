use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use anyhow::Context;
use axum::http::HeaderMap;
use uuid::Uuid;

use super::{clean_str, CodeIntelError};
use crate::code_intel::indexer::{CodeRootKind, CodeRootSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TargetKind {
    Root,
    File,
    Directory,
    Missing,
}

#[derive(Debug, Clone)]
pub(super) struct ResolvedTarget {
    pub(super) root: CodeRootSpec,
    pub(super) target_path: PathBuf,
    pub(super) relative_path: Option<String>,
    pub(super) kind: TargetKind,
}

pub(super) fn resolve_target(
    allowed_roots: &[PathBuf],
    headers: &HeaderMap,
    query_cwd: Option<&str>,
    query_path: Option<&str>,
) -> Result<ResolvedTarget, CodeIntelError> {
    let cwd = request_cwd(headers, query_cwd)?;
    let requested_path = clean_str(query_path);
    let anchor = if let Some(path) = requested_path {
        let path = Path::new(path);
        if path.is_absolute() {
            normalize_absolute_path(path)?
        } else {
            normalize_absolute_path(&cwd.join(path))?
        }
    } else {
        cwd.clone()
    };
    let root = root_spec_for_path(allowed_roots, headers, &anchor)?;
    let target_path = if query_path.is_some() {
        anchor
    } else {
        root.path.clone()
    };
    if !target_path.starts_with(&root.path) {
        return Err(CodeIntelError::bad_request(format!(
            "{} is outside code root {}",
            target_path.display(),
            root.path.display()
        )));
    }
    let relative_path = relative_path(&root.path, &target_path)?;
    let relative_path = relative_path.filter(|path| !path.is_empty());
    let kind = if relative_path.is_none() {
        TargetKind::Root
    } else {
        match std::fs::metadata(&target_path) {
            Ok(metadata) if metadata.is_file() => TargetKind::File,
            Ok(metadata) if metadata.is_dir() => TargetKind::Directory,
            Ok(_) => TargetKind::Missing,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => TargetKind::Missing,
            Err(err) => return Err(CodeIntelError::from(err)),
        }
    };
    Ok(ResolvedTarget {
        root,
        target_path,
        relative_path,
        kind,
    })
}

fn request_cwd(headers: &HeaderMap, query_cwd: Option<&str>) -> Result<PathBuf, CodeIntelError> {
    let cwd = if let Some(cwd) = clean_str(query_cwd) {
        PathBuf::from(cwd)
    } else if let Some(cwd) = header_string(headers, "x-sulion-cwd") {
        PathBuf::from(cwd)
    } else {
        std::env::current_dir().context("read current directory")?
    };
    normalize_absolute_path(&cwd)
}

fn root_spec_for_path(
    allowed_roots: &[PathBuf],
    headers: &HeaderMap,
    path: &Path,
) -> Result<CodeRootSpec, CodeIntelError> {
    for allowed_root in allowed_roots {
        let allowed_root = normalize_absolute_path(allowed_root)?;
        let Ok(rest) = path.strip_prefix(&allowed_root) else {
            continue;
        };
        let Some(root_name) = first_normal_component(rest) else {
            continue;
        };
        let root_path = allowed_root.join(&root_name);
        if !root_path.is_dir() {
            return Err(CodeIntelError::not_found(format!(
                "code root not found: {}",
                root_path.display()
            )));
        }
        let kind = if allowed_root.file_name().and_then(|name| name.to_str()) == Some("repos") {
            CodeRootKind::Repo
        } else {
            CodeRootKind::Workspace
        };
        let workspace_id = if kind == CodeRootKind::Workspace {
            header_string(headers, "x-sulion-workspace-id")
                .and_then(|value| Uuid::parse_str(&value).ok())
        } else {
            None
        };
        return Ok(CodeRootSpec {
            kind,
            name: root_name.to_string_lossy().into_owned(),
            path: root_path,
            repo_name: header_string(headers, "x-sulion-repo")
                .or_else(|| Some(root_name.to_string_lossy().into_owned())),
            workspace_id,
            git_head: header_string(headers, "x-sulion-base-sha"),
        });
    }
    Err(CodeIntelError::bad_request(format!(
        "{} is outside allowed code-intel roots",
        path.display()
    )))
}

fn normalize_absolute_path(path: &Path) -> Result<PathBuf, CodeIntelError> {
    if !path.is_absolute() {
        return Err(CodeIntelError::bad_request(format!(
            "path must be absolute: {}",
            path.display()
        )));
    }
    let mut parts: Vec<OsString> = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) => {
                return Err(CodeIntelError::bad_request("unsupported path prefix"));
            }
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(CodeIntelError::bad_request(format!(
                        "path escapes filesystem root: {}",
                        path.display()
                    )));
                }
            }
            Component::Normal(part) => parts.push(part.to_os_string()),
        }
    }
    let mut normalized = PathBuf::from("/");
    for part in parts {
        normalized.push(part);
    }
    Ok(normalized)
}

fn first_normal_component(path: &Path) -> Option<OsString> {
    path.components().find_map(|component| match component {
        Component::Normal(value) => Some(value.to_os_string()),
        _ => None,
    })
}

fn relative_path(root: &Path, path: &Path) -> Result<Option<String>, CodeIntelError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        CodeIntelError::bad_request(format!(
            "{} is outside code root {}",
            path.display(),
            root.display()
        ))
    })?;
    let value = relative.to_string_lossy().replace('\\', "/");
    Ok(Some(value))
}

fn header_string(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_paths_against_request_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let repos = temp.path().join("repos");
        let repo = repos.join("sulion");
        std::fs::create_dir_all(repo.join("backend/src")).unwrap();
        let cwd = repo.join("backend");
        let headers = HeaderMap::new();

        let target =
            resolve_target(&[repos], &headers, Some(cwd.to_str().unwrap()), Some("src")).unwrap();

        assert_eq!(target.root.name, "sulion");
        assert_eq!(target.kind, TargetKind::Directory);
        assert_eq!(target.relative_path.as_deref(), Some("backend/src"));
    }

    #[test]
    fn rejects_paths_outside_allowed_roots() {
        let temp = tempfile::tempdir().unwrap();
        let repos = temp.path().join("repos");
        std::fs::create_dir_all(&repos).unwrap();
        let headers = HeaderMap::new();

        let err = resolve_target(&[repos], &headers, Some("/tmp"), None).unwrap_err();

        assert_eq!(err.status, axum::http::StatusCode::BAD_REQUEST);
    }
}
