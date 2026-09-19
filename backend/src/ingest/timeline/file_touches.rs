use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::workspace;

use super::{TimelineFileTouch, TimelineToolPair};

#[derive(Debug, Clone)]
pub struct FileTouchContext {
    pub repo_name: String,
    pub repo_root: PathBuf,
    pub working_dir: PathBuf,
}

pub fn extract_file_touches(
    pair: &TimelineToolPair,
    context: Option<&FileTouchContext>,
) -> Vec<TimelineFileTouch> {
    let Some(context) = context else {
        return Vec::new();
    };

    let mut seen = HashSet::new();
    let mut touches = Vec::new();
    let touch_kind = touch_kind_for_pair(pair).to_string();
    let is_write = is_write_pair(pair);

    if let Some(Value::Object(input)) = &pair.input {
        for key in ["path", "file_path", "old_path", "new_path"] {
            if let Some(value) = input.get(key).and_then(Value::as_str) {
                push_touch(
                    &mut touches,
                    &mut seen,
                    context,
                    value,
                    &touch_kind,
                    is_write,
                );
            }
        }

        for key in ["paths", "files"] {
            let Some(Value::Array(values)) = input.get(key) else {
                continue;
            };
            for value in values.iter().filter_map(Value::as_str) {
                push_touch(
                    &mut touches,
                    &mut seen,
                    context,
                    value,
                    &touch_kind,
                    is_write,
                );
            }
        }

        // Agent-agnostic file-edit list produced by the edit /
        // multi_edit / apply_patch / code-mode exec canonicalisers.
        // Each entry carries at minimum a `path`; move operations also
        // carry `old_path`. Always a write, whatever the pair's own
        // kind — an exec snippet that applies a patch edited the file.
        if let Some(Value::Array(entries)) = input.get("file_edits") {
            for entry in entries {
                let Value::Object(obj) = entry else { continue };
                for key in ["path", "old_path"] {
                    if let Some(value) = obj.get(key).and_then(Value::as_str) {
                        push_touch(&mut touches, &mut seen, context, value, "write", true);
                    }
                }
            }
        }
    }

    // Command text is tokenised only when the canonicaliser produced no
    // structured edit list. A code-mode exec that applied a patch already
    // named its files exactly; its snippet is program text, and every
    // `np.random(31)` in it would otherwise read as a path.
    let has_structured_edits = matches!(
        pair.input.as_ref().and_then(|input| input.get("file_edits")),
        Some(Value::Array(entries)) if !entries.is_empty()
    );
    if is_command_pair(pair) && !has_structured_edits {
        if let Some(command) = command_text(pair.input.as_ref()) {
            for token in command
                .split_whitespace()
                .filter_map(clean_command_token)
                .filter(|token| is_path_like_token(token))
            {
                push_touch(
                    &mut touches,
                    &mut seen,
                    context,
                    token,
                    &touch_kind,
                    is_write,
                );
            }
        }
    }

    touches
}

fn push_touch(
    out: &mut Vec<TimelineFileTouch>,
    seen: &mut HashSet<(String, String)>,
    context: &FileTouchContext,
    raw: &str,
    touch_kind: &str,
    is_write: bool,
) {
    let Some(path) = normalize_candidate(context, raw) else {
        return;
    };
    let key = (path.clone(), touch_kind.to_string());
    if !seen.insert(key) {
        return;
    }
    out.push(TimelineFileTouch {
        repo: context.repo_name.clone(),
        path,
        touch_kind: touch_kind.to_string(),
        is_write,
    });
}

fn normalize_candidate(context: &FileTouchContext, raw: &str) -> Option<String> {
    let candidate = raw.trim();
    if candidate.is_empty()
        || candidate.starts_with('-')
        || candidate.starts_with("http://")
        || candidate.starts_with("https://")
        || candidate.starts_with('$')
        || candidate.starts_with("~")
        || candidate.contains('\n')
    {
        return None;
    }

    let rel = if Path::new(candidate).is_absolute() {
        let absolute = PathBuf::from(candidate);
        let stripped = absolute.strip_prefix(&context.repo_root).ok()?;
        stripped.to_string_lossy().into_owned()
    } else {
        if candidate.ends_with('/') {
            return None;
        }
        normalize_relative_candidate(&context.repo_root, &context.repo_root, candidate).or_else(
            || normalize_relative_candidate(&context.repo_root, &context.working_dir, candidate),
        )?
    };

    if rel.is_empty() {
        return None;
    }

    let absolute = context.repo_root.join(&rel);
    if absolute.is_dir() {
        return None;
    }
    Some(rel)
}

fn normalize_relative_candidate(repo_root: &Path, base: &Path, candidate: &str) -> Option<String> {
    let mut current = base.to_path_buf();
    for component in Path::new(candidate).components() {
        use std::path::Component;
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => current.push(segment),
            Component::ParentDir => {
                if current == repo_root || !current.pop() {
                    return None;
                }
            }
            Component::Prefix(_) | Component::RootDir => return None,
        }
    }
    if !current.starts_with(repo_root) {
        return None;
    }
    let rel = current
        .strip_prefix(repo_root)
        .ok()?
        .to_string_lossy()
        .into_owned();
    if rel.is_empty() {
        return None;
    }
    let (_, rel) = workspace::resolve_in_repo(repo_root, &rel).ok()?;
    Some(rel)
}

fn touch_kind_for_pair(pair: &TimelineToolPair) -> &'static str {
    if command_text(pair.input.as_ref()).is_some() {
        return "command";
    }
    match pair.operation_type.as_deref().unwrap_or(pair.name.as_str()) {
        "write" | "edit" | "multi_edit" | "apply_patch" | "create" | "update" | "delete"
        | "add" | "remove" => "write",
        "grep" | "glob" | "find" | "list" | "read" | "fetch" | "get" => "inspect",
        "bash" | "exec" | "exec_command" => "command",
        _ => "inspect",
    }
}

fn is_write_pair(pair: &TimelineToolPair) -> bool {
    matches!(
        pair.operation_type.as_deref().unwrap_or(pair.name.as_str()),
        "write"
            | "edit"
            | "multi_edit"
            | "apply_patch"
            | "create"
            | "update"
            | "delete"
            | "add"
            | "remove"
    )
}

fn is_command_pair(pair: &TimelineToolPair) -> bool {
    command_text(pair.input.as_ref()).is_some()
        || matches!(
            pair.operation_type.as_deref().unwrap_or(pair.name.as_str()),
            "bash" | "exec" | "exec_command"
        )
}

fn command_text(input: Option<&Value>) -> Option<&str> {
    let Value::Object(input) = input? else {
        return None;
    };
    input
        .get("command")
        .and_then(Value::as_str)
        .or_else(|| input.get("cmd").and_then(Value::as_str))
}

fn clean_command_token(token: &str) -> Option<&str> {
    let trimmed = token.trim_matches(|ch: char| {
        matches!(
            ch,
            '"' | '\'' | '`' | ',' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}' | '|'
        )
    });
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Whether a shell token has the shape of a file path. Shell operators,
/// globs, assignments, and program text are excluded outright; the rest
/// must either carry a directory separator or look like a bare file name
/// with a short extension. The ingester has no repo mount, so shape is
/// the only test available here.
fn is_path_like_token(token: &str) -> bool {
    if token.is_empty()
        || !token
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '+' | '@'))
    {
        return false;
    }
    if token.contains('/') {
        return token != "." && token != "..";
    }
    let Some((stem, ext)) = token.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && stem.chars().any(|ch| ch.is_ascii_alphabetic())
        && (1..=5).contains(&ext.len())
        && ext.chars().all(|ch| ch.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn context() -> FileTouchContext {
        FileTouchContext {
            repo_name: "alpha".to_string(),
            repo_root: PathBuf::from("/tmp/alpha"),
            working_dir: PathBuf::from("/tmp/alpha/src"),
        }
    }

    fn pair(operation_type: &str, input: serde_json::Value) -> TimelineToolPair {
        TimelineToolPair {
            id: "tool-1".to_string(),
            name: operation_type.to_string(),
            raw_name: None,
            operation_type: Some(operation_type.to_string()),
            category: None,
            input: Some(input),
            result: None,
            is_error: false,
            is_pending: false,
            file_touches: Vec::new(),
            subagent: None,
        }
    }

    #[test]
    fn extracts_structured_relative_paths_against_working_dir() {
        let pair = pair("read", json!({ "path": "../Cargo.toml" }));
        let touches = extract_file_touches(&pair, Some(&context()));
        assert_eq!(
            touches,
            vec![TimelineFileTouch {
                repo: "alpha".to_string(),
                path: "Cargo.toml".to_string(),
                touch_kind: "inspect".to_string(),
                is_write: false,
            }]
        );
    }

    #[test]
    fn extracts_bash_paths_conservatively() {
        let pair = pair(
            "bash",
            json!({
                "command": "cat /tmp/alpha/src/lib.rs && git diff -- src/main.rs /tmp/out.txt",
            }),
        );
        let touches = extract_file_touches(&pair, Some(&context()));
        assert_eq!(touches.len(), 2);
        assert_eq!(touches[0].path, "src/lib.rs");
        assert_eq!(touches[1].path, "src/main.rs");
    }

    /// Code-mode exec: embedded patch edits touch as writes even though
    /// the pair itself is a command pair. Once the canonicaliser has
    /// named the edited files, the snippet itself is not tokenised — it
    /// is program text, not a shell line.
    #[test]
    fn exec_file_edits_touch_as_writes_and_silence_the_snippet() {
        let pair = pair(
            "apply_patch",
            json!({
                "command": "text(await tools.apply_patch(\"...\")); rng=np.random.default_rng(101); W=rng.random((31,11)); cat /tmp/alpha/src/lib.rs",
                "file_edits": [{ "path": "/tmp/alpha/src/main.rs", "operation": "update" }],
            }),
        );
        let touches = extract_file_touches(&pair, Some(&context()));
        assert_eq!(
            touches,
            vec![TimelineFileTouch {
                repo: "alpha".to_string(),
                path: "src/main.rs".to_string(),
                touch_kind: "write".to_string(),
                is_write: true,
            }]
        );
    }

    #[test]
    fn program_text_and_shell_operators_are_not_paths() {
        for token in [
            "2>/dev/null",
            ">/dev/null",
            "*.cs",
            "!**/bin/**",
            "PYTHONPATH=.",
            "s=open(p).read",
            "open(p,'w').write(s",
            "<noreply@anthropic.com>",
            "xml.etree.ElementTree",
            "ge.run_id",
            "e.plane_code=1",
            "pass.",
            "1.2",
            "3.",
            "np.random.default_rng(101",
            ".",
            "..",
        ] {
            assert!(!is_path_like_token(token), "{token:?} read as a path");
        }
        for token in [
            "src/lib.rs",
            "./run.sh",
            "../Cargo.toml",
            "/tmp/alpha/src/lib.rs",
            "README.md",
            "Cargo.toml",
            "docs/architecture.md",
            "frontend/src",
            "@scope/pkg/index.js",
            "file-name_v2.tsx",
        ] {
            assert!(is_path_like_token(token), "{token:?} not read as a path");
        }
    }

    #[test]
    fn bash_script_lines_yield_only_real_paths() {
        let pair = pair(
            "bash",
            json!({
                "command": "PYTHONPATH=. python3 - <<'EOF' 2>/dev/null\nimport numpy as np\nrng=np.random.default_rng(101)\ns=open('src/lib.rs').read()\nEOF\ncat README.md src/main.rs",
            }),
        );
        let touches = extract_file_touches(&pair, Some(&context()));
        let paths: Vec<&str> = touches.iter().map(|touch| touch.path.as_str()).collect();
        assert_eq!(paths, vec!["README.md", "src/main.rs"]);
    }
}
