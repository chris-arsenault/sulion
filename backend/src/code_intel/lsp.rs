use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, Context};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use url::Url;

use super::indexer::CodeRootSpec;
use super::parser::SourceLanguage;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(750);

#[derive(Clone)]
pub struct LspManager {
    timeout: Duration,
}

impl Default for LspManager {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl LspManager {
    pub fn status(&self) -> SemanticRuntimeStatus {
        let languages = ServerSpec::all()
            .into_iter()
            .map(|spec| spec.status())
            .collect::<Vec<_>>();
        let available = languages.iter().any(|language| language.available);
        SemanticRuntimeStatus {
            available,
            languages,
            reason: if available {
                None
            } else {
                Some("no semantic language servers are available on PATH".to_string())
            },
            timeout_ms: self.timeout.as_millis() as u64,
            fallback: "def and refs fall back to syntactic index results",
        }
    }

    pub async fn definition(
        &self,
        root: &CodeRootSpec,
        file_path: &Path,
        line: i32,
        col: i32,
    ) -> SemanticResponse {
        self.request_locations(root, file_path, line, col, LspRequestKind::Definition)
            .await
    }

    pub async fn references(
        &self,
        root: &CodeRootSpec,
        file_path: &Path,
        line: i32,
        col: i32,
    ) -> SemanticResponse {
        self.request_locations(root, file_path, line, col, LspRequestKind::References)
            .await
    }

    async fn request_locations(
        &self,
        root: &CodeRootSpec,
        file_path: &Path,
        line: i32,
        col: i32,
        kind: LspRequestKind,
    ) -> SemanticResponse {
        let Some(language) = SourceLanguage::from_path(file_path) else {
            return SemanticResponse::unsupported("source language is not supported by code-intel");
        };
        let Some(spec) = ServerSpec::for_language(language) else {
            return SemanticResponse::unsupported(format!(
                "{} has no configured semantic language server",
                language.as_str()
            ));
        };
        if !command_available(spec.command) {
            return SemanticResponse::unavailable(format!(
                "{} is not installed or not on PATH",
                spec.command
            ));
        }
        if line < 1 || col < 1 {
            return SemanticResponse::failed("LSP position must be one-based and positive");
        }
        let source = match tokio::fs::read_to_string(file_path).await {
            Ok(source) => source,
            Err(err) => {
                return SemanticResponse::failed(format!(
                    "read {} for semantic request: {err}",
                    file_path.display()
                ));
            }
        };
        let root = root.clone();
        let file_path = file_path.to_path_buf();
        match tokio::time::timeout(
            self.timeout,
            run_semantic_request(spec, root, file_path, source, line, col, kind),
        )
        .await
        {
            Ok(Ok(locations)) if locations.is_empty() => SemanticResponse::Empty,
            Ok(Ok(locations)) => SemanticResponse::Results(locations),
            Ok(Err(err)) => SemanticResponse::failed(err.to_string()),
            Err(_) => SemanticResponse::failed(format!(
                "{} semantic request timed out after {}ms",
                spec.language,
                self.timeout.as_millis()
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticRuntimeStatus {
    pub available: bool,
    pub languages: Vec<SemanticLanguageRuntimeStatus>,
    pub reason: Option<String>,
    pub timeout_ms: u64,
    pub fallback: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticLanguageRuntimeStatus {
    pub language: &'static str,
    pub command: String,
    pub available: bool,
    pub health: &'static str,
    pub startup: &'static str,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticResponse {
    Results(Vec<LspLocation>),
    Empty,
    Unsupported { reason: String },
    Unavailable { reason: String },
    Failed { reason: String },
}

impl SemanticResponse {
    fn unsupported(reason: impl Into<String>) -> Self {
        Self::Unsupported {
            reason: reason.into(),
        }
    }

    fn unavailable(reason: impl Into<String>) -> Self {
        Self::Unavailable {
            reason: reason.into(),
        }
    }

    fn failed(reason: impl Into<String>) -> Self {
        Self::Failed {
            reason: reason.into(),
        }
    }

    pub fn fallback_warning(&self) -> Option<String> {
        match self {
            Self::Results(_) => None,
            Self::Empty => Some("semantic fallback: language server returned no locations".into()),
            Self::Unsupported { reason }
            | Self::Unavailable { reason }
            | Self::Failed { reason } => Some(format!("semantic fallback: {reason}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspLocation {
    pub path: String,
    pub start_line: i32,
    pub start_col: i32,
    pub end_line: i32,
    pub end_col: i32,
}

#[derive(Clone, Copy)]
struct ServerSpec {
    language: &'static str,
    source_languages: &'static [SourceLanguage],
    command: &'static str,
    args: &'static [&'static str],
    language_id: &'static str,
}

impl ServerSpec {
    fn all() -> Vec<Self> {
        vec![
            Self {
                language: "rust",
                source_languages: &[SourceLanguage::Rust],
                command: "rust-analyzer",
                args: &[],
                language_id: "rust",
            },
            Self {
                language: "typescript",
                source_languages: &[SourceLanguage::TypeScript],
                command: "typescript-language-server",
                args: &["--stdio"],
                language_id: "typescript",
            },
            Self {
                language: "tsx",
                source_languages: &[SourceLanguage::Tsx],
                command: "typescript-language-server",
                args: &["--stdio"],
                language_id: "typescriptreact",
            },
        ]
    }

    fn for_language(language: SourceLanguage) -> Option<Self> {
        Self::all()
            .into_iter()
            .find(|spec| spec.source_languages.contains(&language))
    }

    fn status(self) -> SemanticLanguageRuntimeStatus {
        let available = command_available(self.command);
        SemanticLanguageRuntimeStatus {
            language: self.language,
            command: self.command_line(),
            available,
            health: if available { "available" } else { "missing" },
            startup: "on_demand",
            last_error: if available {
                None
            } else {
                Some(format!("{} is not installed or not on PATH", self.command))
            },
        }
    }

    fn command_line(self) -> String {
        std::iter::once(self.command)
            .chain(self.args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Clone, Copy)]
enum LspRequestKind {
    Definition,
    References,
}

async fn run_semantic_request(
    spec: ServerSpec,
    root: CodeRootSpec,
    file_path: PathBuf,
    source: String,
    line: i32,
    col: i32,
    kind: LspRequestKind,
) -> anyhow::Result<Vec<LspLocation>> {
    let mut client = LspClient::spawn(spec, &root.path).await?;
    let root_uri = file_url(&root.path)?;
    let file_uri = file_url(&file_path)?;
    let initialize = json!({
        "processId": null,
        "rootUri": root_uri,
        "rootPath": root.path.to_string_lossy(),
        "capabilities": {
            "textDocument": {
                "definition": { "linkSupport": true },
                "references": {}
            },
            "workspace": {
                "configuration": true,
                "workspaceFolders": true
            }
        },
        "workspaceFolders": [{
            "uri": root_uri,
            "name": root.name
        }]
    });
    client.request("initialize", initialize).await?;
    client.notify("initialized", json!({})).await?;
    client
        .notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": file_uri,
                    "languageId": spec.language_id,
                    "version": 1,
                    "text": source
                }
            }),
        )
        .await?;

    let params = match kind {
        LspRequestKind::Definition => json!({
            "textDocument": { "uri": file_uri },
            "position": lsp_position(line, col)
        }),
        LspRequestKind::References => json!({
            "textDocument": { "uri": file_uri },
            "position": lsp_position(line, col),
            "context": { "includeDeclaration": true }
        }),
    };
    let result = match kind {
        LspRequestKind::Definition => client.request("textDocument/definition", params).await?,
        LspRequestKind::References => client.request("textDocument/references", params).await?,
    };
    client.shutdown().await;
    let locations = match kind {
        LspRequestKind::Definition => definition_locations(&result),
        LspRequestKind::References => reference_locations(&result),
    };
    Ok(resolve_locations(&root.path, locations))
}

fn lsp_position(line: i32, col: i32) -> Value {
    json!({
        "line": line.saturating_sub(1),
        "character": col.saturating_sub(1)
    })
}

struct LspClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl LspClient {
    async fn spawn(spec: ServerSpec, root: &Path) -> anyhow::Result<Self> {
        let mut command = Command::new(spec.command);
        command
            .args(spec.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .with_context(|| format!("start {}", spec.command_line()))?;
        if let Some(mut stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut sink = Vec::new();
                let _ = stderr.read_to_end(&mut sink).await;
            });
        }
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("{} stdin unavailable", spec.command))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("{} stdout unavailable", spec.command))?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
    }

    async fn request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }))
        .await?;
        loop {
            let message = self.read_message().await?;
            if message.get("id").is_some_and(|value| id_matches(value, id)) {
                if let Some(error) = message.get("error") {
                    return Err(anyhow!("LSP {method} error: {error}"));
                }
                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }
            if is_server_request(&message) {
                self.respond_to_server_request(&message).await?;
            }
        }
    }

    async fn notify(&mut self, method: &str, params: Value) -> anyhow::Result<()> {
        self.send(json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        }))
        .await
    }

    async fn shutdown(mut self) {
        let _ = self.request("shutdown", Value::Null).await;
        let _ = self.notify("exit", Value::Null).await;
        match tokio::time::timeout(SHUTDOWN_TIMEOUT, self.child.wait()).await {
            Ok(_) => {}
            Err(_) => {
                let _ = self.child.kill().await;
            }
        }
    }

    async fn send(&mut self, message: Value) -> anyhow::Result<()> {
        let body = serde_json::to_vec(&message)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        self.stdin.write_all(header.as_bytes()).await?;
        self.stdin.write_all(&body).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn read_message(&mut self) -> anyhow::Result<Value> {
        let mut content_length = None;
        loop {
            let mut line = Vec::new();
            let read = self.stdout.read_until(b'\n', &mut line).await?;
            if read == 0 {
                return Err(anyhow!("language server closed stdout"));
            }
            let line = String::from_utf8_lossy(&line);
            let header = line.trim_end_matches(&['\r', '\n'][..]);
            if header.is_empty() {
                break;
            }
            if let Some(value) = header.strip_prefix("Content-Length:") {
                content_length = Some(value.trim().parse::<usize>()?);
            }
        }
        let len = content_length.ok_or_else(|| anyhow!("missing LSP Content-Length header"))?;
        let mut body = vec![0; len];
        self.stdout.read_exact(&mut body).await?;
        Ok(serde_json::from_slice(&body)?)
    }

    async fn respond_to_server_request(&mut self, message: &Value) -> anyhow::Result<()> {
        let Some(id) = message.get("id").cloned() else {
            return Ok(());
        };
        let result = match message.get("method").and_then(Value::as_str) {
            Some("workspace/configuration") => json!([]),
            _ => Value::Null,
        };
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result
        }))
        .await
    }
}

fn id_matches(value: &Value, id: i64) -> bool {
    value.as_i64() == Some(id)
}

fn is_server_request(message: &Value) -> bool {
    message.get("id").is_some() && message.get("method").is_some()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawLocation {
    uri: String,
    start_line: i32,
    start_col: i32,
    end_line: i32,
    end_col: i32,
}

fn definition_locations(value: &Value) -> Vec<RawLocation> {
    match value {
        Value::Array(items) => items.iter().filter_map(raw_location).collect(),
        Value::Object(_) => raw_location(value).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn reference_locations(value: &Value) -> Vec<RawLocation> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(raw_location)
        .collect()
}

fn raw_location(value: &Value) -> Option<RawLocation> {
    if let Some(uri) = value.get("uri").and_then(Value::as_str) {
        return raw_location_from_range(uri, value.get("range")?);
    }
    if let Some(uri) = value.get("targetUri").and_then(Value::as_str) {
        let range = value
            .get("targetSelectionRange")
            .or_else(|| value.get("targetRange"))?;
        return raw_location_from_range(uri, range);
    }
    None
}

fn raw_location_from_range(uri: &str, range: &Value) -> Option<RawLocation> {
    let start = range.get("start")?;
    let end = range.get("end")?;
    Some(RawLocation {
        uri: uri.to_string(),
        start_line: one_based_i32(start.get("line")?),
        start_col: one_based_i32(start.get("character")?),
        end_line: one_based_i32(end.get("line")?),
        end_col: one_based_i32(end.get("character")?),
    })
}

fn one_based_i32(value: &Value) -> i32 {
    value.as_i64().unwrap_or(0).saturating_add(1) as i32
}

fn resolve_locations(root: &Path, locations: Vec<RawLocation>) -> Vec<LspLocation> {
    let mut out = Vec::new();
    for location in locations {
        let Ok(url) = Url::parse(&location.uri) else {
            continue;
        };
        let Ok(path) = url.to_file_path() else {
            continue;
        };
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        out.push(LspLocation {
            path: relative,
            start_line: location.start_line,
            start_col: location.start_col,
            end_line: location.end_line,
            end_col: location.end_col,
        });
    }
    out.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.start_line.cmp(&right.start_line))
            .then(left.start_col.cmp(&right.start_col))
            .then(left.end_line.cmp(&right.end_line))
            .then(left.end_col.cmp(&right.end_col))
    });
    out.dedup();
    out
}

fn file_url(path: &Path) -> anyhow::Result<String> {
    Url::from_file_path(path)
        .map_err(|_| anyhow!("convert {} to file URL", path.display()))
        .map(|url| url.to_string())
}

fn command_available(command: &str) -> bool {
    if command.contains('/') {
        return is_executable_file(Path::new(command));
    }
    let paths = std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_default();
    command_available_in_paths(command, &paths)
}

fn command_available_in_paths(command: &str, paths: &[PathBuf]) -> bool {
    paths
        .iter()
        .map(|path| path.join(command))
        .any(|path| is_executable_file(&path))
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_location_and_location_link_results() {
        let direct = json!({
            "uri": "file:///repo/src/lib.rs",
            "range": {
                "start": { "line": 4, "character": 8 },
                "end": { "line": 4, "character": 14 }
            }
        });
        let link = json!({
            "targetUri": "file:///repo/src/main.rs",
            "targetSelectionRange": {
                "start": { "line": 1, "character": 0 },
                "end": { "line": 1, "character": 4 }
            }
        });

        assert_eq!(
            definition_locations(&json!([direct, link])),
            vec![
                RawLocation {
                    uri: "file:///repo/src/lib.rs".to_string(),
                    start_line: 5,
                    start_col: 9,
                    end_line: 5,
                    end_col: 15,
                },
                RawLocation {
                    uri: "file:///repo/src/main.rs".to_string(),
                    start_line: 2,
                    start_col: 1,
                    end_line: 2,
                    end_col: 5,
                },
            ]
        );
    }

    #[test]
    fn resolves_file_uri_locations_under_root() {
        let locations = resolve_locations(
            Path::new("/repo"),
            vec![
                RawLocation {
                    uri: "file:///elsewhere/lib.rs".to_string(),
                    start_line: 1,
                    start_col: 1,
                    end_line: 1,
                    end_col: 2,
                },
                RawLocation {
                    uri: "file:///repo/src/lib.rs".to_string(),
                    start_line: 1,
                    start_col: 1,
                    end_line: 1,
                    end_col: 2,
                },
            ],
        );

        assert_eq!(
            locations,
            vec![LspLocation {
                path: "src/lib.rs".to_string(),
                start_line: 1,
                start_col: 1,
                end_line: 1,
                end_col: 2,
            }]
        );
    }

    #[test]
    fn command_detection_uses_explicit_path_list() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("rust-analyzer");
        std::fs::write(&executable, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        assert!(command_available_in_paths(
            "rust-analyzer",
            &[temp.path().to_path_buf()]
        ));
        assert!(!command_available_in_paths(
            "typescript-language-server",
            &[temp.path().to_path_buf()]
        ));
    }
}
