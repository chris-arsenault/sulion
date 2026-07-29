//! Shared plumbing for the PTY-facing HTTP CLIs (`sulion-retrieve`,
//! `sulion-code`). Both talk to a token-authenticated service through the node
//! gateway with the same environment contract, so the environment lookups,
//! scoping headers, and URL construction live here rather than once per CLI.
//!
//! Keeping one copy is not only tidiness: the two had already drifted, and the
//! drift was a bug. `sulion-code` was building request URLs without the base
//! path fix below, so a gateway-prefixed `SULION_CODE_INTEL_URL` silently lost
//! its prefix.

use std::path::Path;

use anyhow::{anyhow, Context};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use url::Url;

/// Roots under which a repo name can be inferred from the working directory.
const REPO_ROOTS: [&str; 2] = ["/home/sulion/repos", "/home/sulion/workspaces"];

pub fn env_required(key: &str) -> anyhow::Result<String> {
    std::env::var(key)
        .map(|value| value.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("{key} is not set"))
}

pub fn env_optional(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// The agent session id under any of the names an agent runtime may export.
pub fn agent_session_id() -> Option<String> {
    env_optional("SULION_AGENT_SESSION_ID")
        .or_else(|| env_optional("SULION_CLAUDE_SESSION_ID"))
        .or_else(|| env_optional("CODEX_SESSION_ID"))
}

pub fn infer_repo(cwd: &str) -> Option<String> {
    let path = Path::new(cwd);
    for prefix in REPO_ROOTS {
        if let Ok(rest) = path.strip_prefix(prefix) {
            if let Some(component) = rest.components().next() {
                let repo = component.as_os_str().to_string_lossy();
                if !repo.is_empty() {
                    return Some(repo.into_owned());
                }
            }
        }
    }
    None
}

/// Adds a scoping header, skipping empty values so an unset variable does not
/// become a blank header the service has to special-case.
pub fn insert_header(
    headers: &mut HeaderMap,
    name: &'static str,
    value: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        headers.insert(name, HeaderValue::from_str(value)?);
    }
    Ok(())
}

/// A header map carrying just the bearer token. Callers add their own scoping
/// headers, which differ per service.
pub fn bearer_headers(token: &str, context: &'static str) -> anyhow::Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}")).context(context)?,
    );
    Ok(headers)
}

/// Joins a request path and query onto a configured base URL.
///
/// The base is forced to end in `/` before joining, because `Url::join`
/// replaces the final path segment otherwise — so a base pointing at a gateway
/// prefix such as `https://host:30081/retrieval` would resolve `/v1/search` to
/// `https://host:30081/v1/search` and drop the prefix. `context` names the
/// environment variable so an invalid value reports which one to fix.
pub fn build_url(
    base_url: &str,
    path: &str,
    pairs: &[(&str, String)],
    context: &'static str,
) -> anyhow::Result<Url> {
    let base_url = format!("{}/", base_url.trim_end_matches('/'));
    let mut url = Url::parse(&base_url)
        .context(context)?
        .join(path.trim_start_matches('/'))?;
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in pairs {
            if !value.trim().is_empty() {
                query.append_pair(key, value);
            }
        }
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_url_preserves_a_configured_base_path() {
        let url = build_url(
            "https://192.168.66.3:30081/retrieval",
            "/v1/search",
            &[("q", "node migration".to_string())],
            "invalid base",
        )
        .unwrap();
        assert_eq!(
            url.as_str(),
            "https://192.168.66.3:30081/retrieval/v1/search?q=node+migration"
        );
    }

    #[test]
    fn build_url_handles_a_bare_origin_and_trailing_slashes() {
        // Asserts path and origin rather than the whole string: with no query
        // pairs the URL keeps a bare `?`, which both CLIs have always emitted.
        for base in ["http://sulion-retrieval:8083", "http://sulion-retrieval:8083/"] {
            let url = build_url(base, "/v1/context", &[], "invalid base").unwrap();
            assert_eq!(url.path(), "/v1/context");
            assert_eq!(url.host_str(), Some("sulion-retrieval"));
            assert_eq!(url.port(), Some(8083));
        }
    }

    #[test]
    fn build_url_skips_blank_query_values() {
        let url = build_url(
            "http://host/code-intel",
            "/v1/find",
            &[("cwd", "  ".to_string()), ("budget", "small".to_string())],
            "invalid base",
        )
        .unwrap();
        assert_eq!(url.as_str(), "http://host/code-intel/v1/find?budget=small");
    }

    #[test]
    fn infer_repo_reads_the_first_component_under_a_known_root() {
        assert_eq!(
            infer_repo("/home/sulion/repos/sulion/backend/src"),
            Some("sulion".to_string())
        );
        assert_eq!(
            infer_repo("/home/sulion/workspaces/feature-x"),
            Some("feature-x".to_string())
        );
        assert_eq!(infer_repo("/tmp/elsewhere"), None);
        assert_eq!(infer_repo("/home/sulion/repos"), None);
    }

    #[test]
    fn insert_header_skips_blank_values() {
        let mut headers = HeaderMap::new();
        insert_header(&mut headers, "x-sulion-repo", Some("sulion")).unwrap();
        insert_header(&mut headers, "x-sulion-cwd", Some("   ")).unwrap();
        insert_header(&mut headers, "x-sulion-pty-id", None).unwrap();
        assert_eq!(headers.get("x-sulion-repo").unwrap(), "sulion");
        assert!(headers.get("x-sulion-cwd").is_none());
        assert!(headers.get("x-sulion-pty-id").is_none());
    }
}
