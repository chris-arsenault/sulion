use super::*;

pub(super) struct OpenDocument {
    pub(super) version: i32,
    pub(super) fingerprint: u64,
}

pub(super) fn initialization_options(spec: ServerSpec) -> Value {
    match spec.server_family {
        "rust" => json!({
            "cargo": {
                "allFeatures": true,
                "buildScripts": { "enable": true }
            },
            "procMacro": { "enable": true },
            "checkOnSave": true,
            "check": { "command": "check", "allTargets": true }
        }),
        _ => Value::Null,
    }
}

pub(super) struct LspTransport {
    stdin: Arc<AsyncMutex<ChildStdin>>,
    pending: PendingResponses,
    next_id: AtomicI64,
    _child: Child,
    _reader: JoinHandle<()>,
}

pub(super) type PendingResponse = oneshot::Sender<anyhow::Result<Value>>;
pub(super) type PendingResponses = Arc<AsyncMutex<HashMap<i64, PendingResponse>>>;

impl LspTransport {
    pub(super) async fn spawn(spec: ServerSpec, root: &Path) -> anyhow::Result<Self> {
        let mut command = Command::new(spec.command);
        command
            .args(spec.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        apply_runtime_environment(&mut command, spec, root)?;
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
        let stdin = Arc::new(AsyncMutex::new(stdin));
        let pending = Arc::new(AsyncMutex::new(HashMap::new()));
        let reader = tokio::spawn(read_loop(
            BufReader::new(stdout),
            stdin.clone(),
            pending.clone(),
        ));
        Ok(Self {
            stdin,
            pending,
            next_id: AtomicI64::new(1),
            _child: child,
            _reader: reader,
        })
    }

    pub(super) async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        let send_result = send_message(
            &self.stdin,
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params
            }),
        )
        .await;
        if let Err(err) = send_result {
            self.pending.lock().await.remove(&id);
            return Err(err);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(anyhow!(
                "language server closed response channel for {method}"
            )),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(anyhow!(
                    "{method} timed out after {}ms",
                    timeout.as_millis()
                ))
            }
        }
    }

    pub(super) async fn notify(&self, method: &str, params: Value) -> anyhow::Result<()> {
        send_message(
            &self.stdin,
            json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params
            }),
        )
        .await
    }
}

pub(super) async fn read_loop(
    mut stdout: BufReader<ChildStdout>,
    stdin: Arc<AsyncMutex<ChildStdin>>,
    pending: PendingResponses,
) {
    loop {
        let message = match read_message(&mut stdout).await {
            Ok(message) => message,
            Err(err) => {
                fail_all_pending(&pending, err).await;
                return;
            }
        };
        if is_server_request(&message) {
            let _ = respond_to_server_request(&stdin, &message).await;
            continue;
        }
        let Some(id) = message.get("id").and_then(Value::as_i64) else {
            continue;
        };
        if let Some(tx) = pending.lock().await.remove(&id) {
            let result = if let Some(error) = message.get("error") {
                Err(anyhow!("LSP request {id} error: {error}"))
            } else {
                Ok(message.get("result").cloned().unwrap_or(Value::Null))
            };
            let _ = tx.send(result);
        }
    }
}

pub(super) async fn fail_all_pending(pending: &PendingResponses, err: anyhow::Error) {
    let reason = format!("language server output closed: {err}");
    let mut pending = pending.lock().await;
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(anyhow!(reason.clone())));
    }
}

pub(super) async fn respond_to_server_request(
    stdin: &Arc<AsyncMutex<ChildStdin>>,
    message: &Value,
) -> anyhow::Result<()> {
    let Some(id) = message.get("id").cloned() else {
        return Ok(());
    };
    let result = match message.get("method").and_then(Value::as_str) {
        Some("workspace/configuration") => workspace_configuration_result(message),
        Some("workspace/applyEdit") => json!({ "applied": false }),
        _ => Value::Null,
    };
    send_message(
        stdin,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result
        }),
    )
    .await
}

pub(super) fn workspace_configuration_result(message: &Value) -> Value {
    let count = message
        .pointer("/params/items")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    Value::Array((0..count).map(|_| json!({})).collect())
}

pub(super) async fn send_message(
    stdin: &Arc<AsyncMutex<ChildStdin>>,
    message: Value,
) -> anyhow::Result<()> {
    let body = serde_json::to_vec(&message)?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut stdin = stdin.lock().await;
    stdin.write_all(header.as_bytes()).await?;
    stdin.write_all(&body).await?;
    stdin.flush().await?;
    Ok(())
}

pub(super) async fn read_message(stdout: &mut BufReader<ChildStdout>) -> anyhow::Result<Value> {
    let mut content_length = None;
    loop {
        let mut line = Vec::new();
        let read = stdout.read_until(b'\n', &mut line).await?;
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
    stdout.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body)?)
}

pub(super) fn is_server_request(message: &Value) -> bool {
    message.get("id").is_some() && message.get("method").is_some()
}

pub(super) fn lsp_position(source: &str, line: i32, col: i32) -> Value {
    json!({
        "line": line.saturating_sub(1),
        "character": utf16_character_for_one_based_byte_col(source, line, col)
    })
}

pub(super) fn utf16_character_for_one_based_byte_col(source: &str, line: i32, col: i32) -> usize {
    if line < 1 || col < 1 {
        return 0;
    }
    let Some(line_text) = line_text(source, line as usize) else {
        return col.saturating_sub(1) as usize;
    };
    let target = (col - 1) as usize;
    let target = target.min(line_text.len());
    let boundary = if line_text.is_char_boundary(target) {
        target
    } else {
        (0..target)
            .rev()
            .find(|idx| line_text.is_char_boundary(*idx))
            .unwrap_or(0)
    };
    line_text[..boundary].encode_utf16().count()
}

pub(super) fn line_text(source: &str, target_line: usize) -> Option<&str> {
    let mut line = 1;
    let mut start = 0;
    for (idx, byte) in source.bytes().enumerate() {
        if line == target_line && byte == b'\n' {
            return Some(&source[start..idx]);
        }
        if byte == b'\n' {
            line += 1;
            start = idx + 1;
        }
    }
    if line == target_line {
        Some(&source[start..])
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RawLocation {
    pub(super) uri: String,
    pub(super) start_line: i32,
    pub(super) start_col: i32,
    pub(super) end_line: i32,
    pub(super) end_col: i32,
}

pub(super) fn definition_locations(value: &Value) -> Vec<RawLocation> {
    match value {
        Value::Array(items) => items.iter().filter_map(raw_location).collect(),
        Value::Object(_) => raw_location(value).into_iter().collect(),
        _ => Vec::new(),
    }
}

pub(super) fn reference_locations(value: &Value) -> Vec<RawLocation> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(raw_location)
        .collect()
}

pub(super) fn raw_location(value: &Value) -> Option<RawLocation> {
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

pub(super) fn raw_location_from_range(uri: &str, range: &Value) -> Option<RawLocation> {
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

pub(super) fn one_based_i32(value: &Value) -> i32 {
    value.as_i64().unwrap_or(0).saturating_add(1) as i32
}

pub(super) fn resolve_locations(root: &Path, locations: Vec<RawLocation>) -> Vec<LspLocation> {
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

pub(super) fn file_url(path: &Path) -> anyhow::Result<String> {
    Url::from_file_path(path)
        .map_err(|_| anyhow!("convert {} to file URL", path.display()))
        .map(|url| url.to_string())
}

pub(super) fn apply_runtime_environment(
    command: &mut Command,
    spec: ServerSpec,
    root: &Path,
) -> anyhow::Result<()> {
    if spec.server_family == "rust" {
        let cache_dir = cache_dir_for_root(root, spec.server_family);
        std::fs::create_dir_all(&cache_dir)
            .with_context(|| format!("create {}", cache_dir.display()))?;
        command.env("CARGO_TARGET_DIR", cache_dir.join("target"));
    }
    Ok(())
}

pub(super) fn cache_dir_for_root(root: &Path, language: &str) -> PathBuf {
    let base = std::env::var_os(CODE_INTEL_CACHE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CACHE_DIR));
    base.join(language).join(stable_path_hash(root))
}

pub(super) fn stable_path_hash(root: &Path) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    root.to_string_lossy().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub(super) fn source_fingerprint(source: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

pub(super) fn env_duration(key: &str, default: Duration, min: Duration, max: Duration) -> Duration {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(default)
        .clamp(min, max)
}

pub(super) fn env_usize(key: &str, default: usize, min: usize, max: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(default)
        .clamp(min, max)
}

pub(super) fn command_available(command: &str) -> bool {
    if command.contains('/') {
        return is_executable_file(Path::new(command));
    }
    let paths = std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_default();
    command_available_in_paths(command, &paths)
}

pub(super) fn command_available_in_paths(command: &str, paths: &[PathBuf]) -> bool {
    paths
        .iter()
        .map(|path| path.join(command))
        .any(|path| is_executable_file(&path))
}

pub(super) fn is_executable_file(path: &Path) -> bool {
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
