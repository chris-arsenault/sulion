use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{oneshot, Mutex as AsyncMutex};
use tokio::task::JoinHandle;
use url::Url;

use super::indexer::CodeRootSpec;
use super::parser::SourceLanguage;

const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_WARMUP_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const DEFAULT_MAX_ACTIVE_SERVERS: usize = 6;
const LSP_REQUEST_SECONDS_ENV: &str = "SULION_CODE_INTEL_LSP_REQUEST_SECONDS";
const LSP_WARMUP_SECONDS_ENV: &str = "SULION_CODE_INTEL_LSP_WARMUP_SECONDS";
const LSP_IDLE_SECONDS_ENV: &str = "SULION_CODE_INTEL_LSP_IDLE_SECONDS";
const LSP_MAX_SERVERS_ENV: &str = "SULION_CODE_INTEL_LSP_MAX_SERVERS";
const CODE_INTEL_CACHE_ENV: &str = "SULION_CODE_INTEL_CACHE_DIR";
const DEFAULT_CACHE_DIR: &str = "/var/lib/sulion-code-intel/cache";

#[derive(Clone)]
pub struct LspManager {
    inner: Arc<LspManagerInner>,
}

struct LspManagerInner {
    request_timeout: Duration,
    warmup_timeout: Duration,
    idle_timeout: Duration,
    max_active_servers: usize,
    clients: AsyncMutex<HashMap<LspClientKey, Arc<LspClientSlot>>>,
    health: StdMutex<HashMap<LspClientKey, LspRuntimeRecord>>,
}

impl Default for LspManager {
    fn default() -> Self {
        Self {
            inner: Arc::new(LspManagerInner {
                request_timeout: env_duration(
                    LSP_REQUEST_SECONDS_ENV,
                    DEFAULT_REQUEST_TIMEOUT,
                    Duration::from_secs(1),
                    Duration::from_secs(60),
                ),
                warmup_timeout: env_duration(
                    LSP_WARMUP_SECONDS_ENV,
                    DEFAULT_WARMUP_TIMEOUT,
                    Duration::from_secs(10),
                    Duration::from_secs(10 * 60),
                ),
                idle_timeout: env_duration(
                    LSP_IDLE_SECONDS_ENV,
                    DEFAULT_IDLE_TIMEOUT,
                    Duration::from_secs(60),
                    Duration::from_secs(24 * 60 * 60),
                ),
                max_active_servers: env_usize(
                    LSP_MAX_SERVERS_ENV,
                    DEFAULT_MAX_ACTIVE_SERVERS,
                    1,
                    32,
                ),
                clients: AsyncMutex::new(HashMap::new()),
                health: StdMutex::new(HashMap::new()),
            }),
        }
    }
}

impl LspManager {
    pub fn status(&self) -> SemanticRuntimeStatus {
        let health = self.inner.health.lock().unwrap();
        let active_servers = health
            .values()
            .filter(|record| record.state.is_active())
            .count();
        let languages = ServerSpec::all()
            .into_iter()
            .map(|spec| spec.status(&health))
            .collect::<Vec<_>>();
        let available = languages.iter().any(|language| language.available);
        SemanticRuntimeStatus {
            available,
            languages,
            reason: if available {
                None
            } else {
                Some("no complete semantic language-server runtime is available".to_string())
            },
            timeout_ms: self.inner.request_timeout.as_millis() as u64,
            warmup_timeout_ms: self.inner.warmup_timeout.as_millis() as u64,
            idle_timeout_ms: self.inner.idle_timeout.as_millis() as u64,
            active_servers,
            max_active_servers: self.inner.max_active_servers,
            fallback: "syntactic fallback is explicit when semantic resolution is unavailable",
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
        let missing = spec.missing_commands();
        if !missing.is_empty() {
            return SemanticResponse::unavailable(format!(
                "{} semantic runtime is missing required command(s): {}",
                spec.language,
                missing.join(", ")
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

        let key = LspClientKey::new(root, spec);
        let client = match self.client_for(key.clone(), spec, root).await {
            Ok(client) => client,
            Err(err) => return SemanticResponse::failed(err.to_string()),
        };

        let result = {
            let mut server = client.lock().await;
            server
                .request_locations(LspLocationRequest {
                    file_path,
                    source: &source,
                    line,
                    col,
                    kind,
                    request_timeout: self.inner.request_timeout,
                    warmup_timeout: self.inner.warmup_timeout,
                })
                .await
        };

        match result {
            Ok(locations) if locations.is_empty() => SemanticResponse::Empty,
            Ok(locations) => {
                self.inner.set_health(&key, LspRuntimeState::Ready, None);
                SemanticResponse::Results(locations)
            }
            Err(err) => {
                let reason = err.to_string();
                self.inner
                    .set_health(&key, LspRuntimeState::Failed, Some(reason.clone()));
                self.inner.reset_client(&key).await;
                SemanticResponse::failed(reason)
            }
        }
    }

    async fn client_for(
        &self,
        key: LspClientKey,
        spec: ServerSpec,
        root: &CodeRootSpec,
    ) -> anyhow::Result<Arc<AsyncMutex<RootLanguageServer>>> {
        if let Some(slot) = self.inner.client_slot(&key).await {
            let client = slot.client.lock().await;
            if let Some(client) = client.as_ref() {
                slot.touch();
                return Ok(client.clone());
            }
        }

        self.inner.ensure_capacity_for(&key).await?;
        let slot = self.inner.client_slot_or_insert(key.clone()).await;
        let mut client = slot.client.lock().await;
        if let Some(client) = client.as_ref() {
            slot.touch();
            return Ok(client.clone());
        }
        self.inner.set_health(&key, LspRuntimeState::Warming, None);
        let server = match tokio::time::timeout(
            self.inner.warmup_timeout,
            RootLanguageServer::spawn(spec, root, self.inner.warmup_timeout),
        )
        .await
        {
            Ok(Ok(server)) => server,
            Ok(Err(err)) => {
                let reason = err.to_string();
                self.inner
                    .set_health(&key, LspRuntimeState::Failed, Some(reason.clone()));
                drop(client);
                self.inner.reset_client(&key).await;
                return Err(anyhow!(reason));
            }
            Err(_) => {
                let reason = format!(
                    "{} semantic server warmup timed out after {}ms",
                    spec.language,
                    self.inner.warmup_timeout.as_millis()
                );
                self.inner
                    .set_health(&key, LspRuntimeState::Failed, Some(reason.clone()));
                drop(client);
                self.inner.reset_client(&key).await;
                return Err(anyhow!(reason));
            }
        };
        let server = Arc::new(AsyncMutex::new(server));
        *client = Some(server.clone());
        slot.touch();
        self.inner.set_health(&key, LspRuntimeState::Ready, None);
        Ok(server)
    }
}

impl LspManagerInner {
    async fn client_slot(&self, key: &LspClientKey) -> Option<Arc<LspClientSlot>> {
        self.clients.lock().await.get(key).cloned()
    }

    async fn client_slot_or_insert(&self, key: LspClientKey) -> Arc<LspClientSlot> {
        let mut clients = self.clients.lock().await;
        clients
            .entry(key)
            .or_insert_with(|| Arc::new(LspClientSlot::new()))
            .clone()
    }

    async fn ensure_capacity_for(&self, key: &LspClientKey) -> anyhow::Result<()> {
        self.evict_expired_clients(Some(key)).await;
        loop {
            let at_capacity = {
                let clients = self.clients.lock().await;
                !clients.contains_key(key) && clients.len() >= self.max_active_servers
            };
            if !at_capacity {
                return Ok(());
            }
            if !self.evict_oldest_client(Some(key)).await {
                anyhow::bail!(
                    "semantic runtime capacity reached: {} active server(s), max {}; retry after an active request finishes or increase {}",
                    self.active_server_count(),
                    self.max_active_servers,
                    LSP_MAX_SERVERS_ENV
                );
            }
        }
    }

    async fn evict_expired_clients(&self, protected: Option<&LspClientKey>) {
        let now = Instant::now();
        let keys = {
            let clients = self.clients.lock().await;
            clients
                .iter()
                .filter(|(key, slot)| {
                    protected != Some(*key)
                        && slot.eviction_score().is_some_and(|last_used| {
                            now.duration_since(last_used) >= self.idle_timeout
                        })
                })
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>()
        };
        self.remove_clients(keys).await;
    }

    async fn evict_oldest_client(&self, protected: Option<&LspClientKey>) -> bool {
        let key = {
            let clients = self.clients.lock().await;
            clients
                .iter()
                .filter(|(key, _)| protected != Some(*key))
                .filter_map(|(key, slot)| {
                    slot.eviction_score()
                        .map(|last_used| (key.clone(), last_used))
                })
                .min_by_key(|(_, last_used)| *last_used)
                .map(|(key, _)| key)
        };
        match key {
            Some(key) => {
                self.remove_clients(vec![key]).await;
                true
            }
            None => false,
        }
    }

    async fn remove_clients(&self, keys: Vec<LspClientKey>) {
        if keys.is_empty() {
            return;
        }
        let removed = {
            let mut clients = self.clients.lock().await;
            keys.into_iter()
                .filter_map(|key| clients.remove(&key).map(|slot| (key, slot)))
                .collect::<Vec<_>>()
        };
        for (key, slot) in removed {
            slot.clear().await;
            self.health.lock().unwrap().remove(&key);
        }
    }

    fn active_server_count(&self) -> usize {
        self.health
            .lock()
            .unwrap()
            .values()
            .filter(|record| record.state.is_active())
            .count()
    }

    fn set_health(&self, key: &LspClientKey, state: LspRuntimeState, last_error: Option<String>) {
        self.health
            .lock()
            .unwrap()
            .insert(key.clone(), LspRuntimeRecord { state, last_error });
    }

    async fn reset_client(&self, key: &LspClientKey) {
        let slot = self.clients.lock().await.remove(key);
        if let Some(slot) = slot {
            slot.clear().await;
        }
    }
}

struct LspClientSlot {
    client: AsyncMutex<Option<Arc<AsyncMutex<RootLanguageServer>>>>,
    last_used: StdMutex<Instant>,
}

impl LspClientSlot {
    fn new() -> Self {
        Self {
            client: AsyncMutex::new(None),
            last_used: StdMutex::new(Instant::now()),
        }
    }

    fn touch(&self) {
        *self.last_used.lock().unwrap() = Instant::now();
    }

    fn eviction_score(&self) -> Option<Instant> {
        let client = self.client.try_lock().ok()?;
        client.as_ref()?;
        Some(*self.last_used.lock().unwrap())
    }

    async fn clear(&self) {
        *self.client.lock().await = None;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticRuntimeStatus {
    pub available: bool,
    pub languages: Vec<SemanticLanguageRuntimeStatus>,
    pub reason: Option<String>,
    pub timeout_ms: u64,
    pub warmup_timeout_ms: u64,
    pub idle_timeout_ms: u64,
    pub active_servers: usize,
    pub max_active_servers: usize,
    pub fallback: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticLanguageRuntimeStatus {
    pub language: &'static str,
    pub command: String,
    pub available: bool,
    pub health: String,
    pub startup: &'static str,
    pub active_roots: usize,
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
    server_family: &'static str,
    source_languages: &'static [SourceLanguage],
    command: &'static str,
    args: &'static [&'static str],
    required_commands: &'static [&'static str],
    language_id: &'static str,
}

impl ServerSpec {
    fn all() -> Vec<Self> {
        vec![
            Self {
                language: "rust",
                server_family: "rust",
                source_languages: &[SourceLanguage::Rust],
                command: "rust-analyzer",
                args: &[],
                required_commands: &["cargo", "rustc"],
                language_id: "rust",
            },
            Self {
                language: "typescript",
                server_family: "typescript",
                source_languages: &[SourceLanguage::TypeScript],
                command: "typescript-language-server",
                args: &["--stdio"],
                required_commands: &["node"],
                language_id: "typescript",
            },
            Self {
                language: "tsx",
                server_family: "typescript",
                source_languages: &[SourceLanguage::Tsx],
                command: "typescript-language-server",
                args: &["--stdio"],
                required_commands: &["node"],
                language_id: "typescriptreact",
            },
            Self {
                language: "javascript",
                server_family: "typescript",
                source_languages: &[SourceLanguage::JavaScript],
                command: "typescript-language-server",
                args: &["--stdio"],
                required_commands: &["node"],
                language_id: "javascript",
            },
        ]
    }

    fn for_language(language: SourceLanguage) -> Option<Self> {
        Self::all()
            .into_iter()
            .find(|spec| spec.source_languages.contains(&language))
    }

    fn missing_commands(self) -> Vec<&'static str> {
        std::iter::once(self.command)
            .chain(self.required_commands.iter().copied())
            .filter(|command| !command_available(command))
            .collect()
    }

    fn status(
        self,
        records: &HashMap<LspClientKey, LspRuntimeRecord>,
    ) -> SemanticLanguageRuntimeStatus {
        let missing = self.missing_commands();
        if !missing.is_empty() {
            return SemanticLanguageRuntimeStatus {
                language: self.language,
                command: self.command_line(),
                available: false,
                health: "missing".to_string(),
                startup: "lazy_persistent",
                active_roots: 0,
                last_error: Some(format!(
                    "missing required semantic runtime command(s): {}",
                    missing.join(", ")
                )),
            };
        }

        let matching = records
            .iter()
            .filter(|(key, _)| key.server_family == self.server_family)
            .map(|(_, record)| record)
            .collect::<Vec<_>>();
        let active_roots = matching
            .iter()
            .filter(|record| record.state.is_active())
            .count();
        let health = if matching
            .iter()
            .any(|record| record.state == LspRuntimeState::Ready)
        {
            "ready"
        } else if matching
            .iter()
            .any(|record| record.state == LspRuntimeState::Warming)
        {
            "warming"
        } else if matching
            .iter()
            .any(|record| record.state == LspRuntimeState::Failed)
        {
            "failed"
        } else {
            "available"
        };
        let last_error = matching
            .iter()
            .filter_map(|record| record.last_error.clone())
            .next_back();
        SemanticLanguageRuntimeStatus {
            language: self.language,
            command: self.command_line(),
            available: true,
            health: health.to_string(),
            startup: "lazy_persistent",
            active_roots,
            last_error,
        }
    }

    fn command_line(self) -> String {
        std::iter::once(self.command)
            .chain(self.args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn language_id_for_path(self, path: &Path) -> &'static str {
        if self.server_family != "typescript" {
            return self.language_id;
        }
        match path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("tsx") => "typescriptreact",
            Some("jsx") => "javascriptreact",
            Some("js" | "mjs" | "cjs") => "javascript",
            _ => "typescript",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LspClientKey {
    root: PathBuf,
    server_family: &'static str,
}

impl LspClientKey {
    fn new(root: &CodeRootSpec, spec: ServerSpec) -> Self {
        Self {
            root: root.path.clone(),
            server_family: spec.server_family,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LspRuntimeState {
    Warming,
    Ready,
    Failed,
}

impl LspRuntimeState {
    fn is_active(self) -> bool {
        matches!(self, Self::Warming | Self::Ready)
    }
}

#[derive(Debug, Clone)]
struct LspRuntimeRecord {
    state: LspRuntimeState,
    last_error: Option<String>,
}

#[derive(Clone, Copy)]
enum LspRequestKind {
    Definition,
    References,
}

struct LspLocationRequest<'a> {
    file_path: &'a Path,
    source: &'a str,
    line: i32,
    col: i32,
    kind: LspRequestKind,
    request_timeout: Duration,
    warmup_timeout: Duration,
}

struct RootLanguageServer {
    spec: ServerSpec,
    root: PathBuf,
    transport: LspTransport,
    open_documents: HashMap<String, OpenDocument>,
    semantic_request_count: u64,
}

impl RootLanguageServer {
    async fn spawn(
        spec: ServerSpec,
        root: &CodeRootSpec,
        warmup_timeout: Duration,
    ) -> anyhow::Result<Self> {
        let transport = LspTransport::spawn(spec, &root.path).await?;
        let root_uri = file_url(&root.path)?;
        let initialize = json!({
            "processId": null,
            "rootUri": root_uri,
            "rootPath": root.path.to_string_lossy(),
            "capabilities": {
                "textDocument": {
                    "definition": { "linkSupport": true },
                    "references": {},
                    "synchronization": {
                        "dynamicRegistration": false,
                        "didSave": false,
                        "willSave": false,
                        "willSaveWaitUntil": false
                    }
                },
                "workspace": {
                    "configuration": true,
                    "workspaceFolders": true,
                    "didChangeConfiguration": { "dynamicRegistration": false },
                    "didChangeWatchedFiles": { "dynamicRegistration": false },
                    "symbol": { "dynamicRegistration": false }
                },
                "window": {
                    "workDoneProgress": true
                }
            },
            "initializationOptions": initialization_options(spec),
            "workspaceFolders": [{
                "uri": root_uri,
                "name": root.name
            }]
        });
        transport
            .request("initialize", initialize, warmup_timeout)
            .await?;
        transport.notify("initialized", json!({})).await?;
        Ok(Self {
            spec,
            root: root.path.clone(),
            transport,
            open_documents: HashMap::new(),
            semantic_request_count: 0,
        })
    }

    async fn request_locations(
        &mut self,
        request: LspLocationRequest<'_>,
    ) -> anyhow::Result<Vec<LspLocation>> {
        let file_uri = file_url(request.file_path)?;
        self.sync_document(request.file_path, &file_uri, request.source)
            .await?;
        let position = lsp_position(request.source, request.line, request.col);
        let params = match request.kind {
            LspRequestKind::Definition => json!({
                "textDocument": { "uri": file_uri },
                "position": position
            }),
            LspRequestKind::References => json!({
                "textDocument": { "uri": file_uri },
                "position": position,
                "context": { "includeDeclaration": true }
            }),
        };
        let method = match request.kind {
            LspRequestKind::Definition => "textDocument/definition",
            LspRequestKind::References => "textDocument/references",
        };
        let timeout = if self.semantic_request_count == 0 {
            request.warmup_timeout
        } else {
            request.request_timeout
        };
        let result = self.transport.request(method, params, timeout).await?;
        self.semantic_request_count += 1;
        let locations = match request.kind {
            LspRequestKind::Definition => definition_locations(&result),
            LspRequestKind::References => reference_locations(&result),
        };
        Ok(resolve_locations(&self.root, locations))
    }

    async fn sync_document(
        &mut self,
        file_path: &Path,
        file_uri: &str,
        source: &str,
    ) -> anyhow::Result<()> {
        let fingerprint = source_fingerprint(source);
        match self.open_documents.get_mut(file_uri) {
            Some(document) if document.fingerprint == fingerprint => Ok(()),
            Some(document) => {
                document.version += 1;
                document.fingerprint = fingerprint;
                self.transport
                    .notify(
                        "textDocument/didChange",
                        json!({
                            "textDocument": {
                                "uri": file_uri,
                                "version": document.version
                            },
                            "contentChanges": [{ "text": source }]
                        }),
                    )
                    .await
            }
            None => {
                self.transport
                    .notify(
                        "textDocument/didOpen",
                        json!({
                            "textDocument": {
                                "uri": file_uri,
                                "languageId": self.spec.language_id_for_path(file_path),
                                "version": 1,
                                "text": source
                            }
                        }),
                    )
                    .await?;
                self.open_documents.insert(
                    file_uri.to_string(),
                    OpenDocument {
                        version: 1,
                        fingerprint,
                    },
                );
                Ok(())
            }
        }
    }
}

struct OpenDocument {
    version: i32,
    fingerprint: u64,
}

fn initialization_options(spec: ServerSpec) -> Value {
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

struct LspTransport {
    stdin: Arc<AsyncMutex<ChildStdin>>,
    pending: PendingResponses,
    next_id: AtomicI64,
    _child: Child,
    _reader: JoinHandle<()>,
}

type PendingResponse = oneshot::Sender<anyhow::Result<Value>>;
type PendingResponses = Arc<AsyncMutex<HashMap<i64, PendingResponse>>>;

impl LspTransport {
    async fn spawn(spec: ServerSpec, root: &Path) -> anyhow::Result<Self> {
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

    async fn request(
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

    async fn notify(&self, method: &str, params: Value) -> anyhow::Result<()> {
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

async fn read_loop(
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

async fn fail_all_pending(pending: &PendingResponses, err: anyhow::Error) {
    let reason = format!("language server output closed: {err}");
    let mut pending = pending.lock().await;
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(anyhow!(reason.clone())));
    }
}

async fn respond_to_server_request(
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

fn workspace_configuration_result(message: &Value) -> Value {
    let count = message
        .pointer("/params/items")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    Value::Array((0..count).map(|_| json!({})).collect())
}

async fn send_message(stdin: &Arc<AsyncMutex<ChildStdin>>, message: Value) -> anyhow::Result<()> {
    let body = serde_json::to_vec(&message)?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut stdin = stdin.lock().await;
    stdin.write_all(header.as_bytes()).await?;
    stdin.write_all(&body).await?;
    stdin.flush().await?;
    Ok(())
}

async fn read_message(stdout: &mut BufReader<ChildStdout>) -> anyhow::Result<Value> {
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

fn is_server_request(message: &Value) -> bool {
    message.get("id").is_some() && message.get("method").is_some()
}

fn lsp_position(source: &str, line: i32, col: i32) -> Value {
    json!({
        "line": line.saturating_sub(1),
        "character": utf16_character_for_one_based_byte_col(source, line, col)
    })
}

fn utf16_character_for_one_based_byte_col(source: &str, line: i32, col: i32) -> usize {
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

fn line_text(source: &str, target_line: usize) -> Option<&str> {
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

fn apply_runtime_environment(
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

fn cache_dir_for_root(root: &Path, language: &str) -> PathBuf {
    let base = std::env::var_os(CODE_INTEL_CACHE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CACHE_DIR));
    base.join(language).join(stable_path_hash(root))
}

fn stable_path_hash(root: &Path) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    root.to_string_lossy().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn source_fingerprint(source: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

fn env_duration(key: &str, default: Duration, min: Duration, max: Duration) -> Duration {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(default)
        .clamp(min, max)
}

fn env_usize(key: &str, default: usize, min: usize, max: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(default)
        .clamp(min, max)
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

    const FAKE_LSP_SCRIPT: &str = r#"
starts="$(pwd)/.fake_lsp_starts"
count="$(cat "$starts" 2>/dev/null || echo 0)"
count=$((count + 1))
printf '%s\n' "$count" > "$starts"
send() {
  body="$1"
  len="$(printf '%s' "$body" | wc -c | tr -d ' ')"
  printf 'Content-Length: %s\r\n\r\n%s' "$len" "$body"
}
while IFS= read -r header; do
  len="$(printf '%s' "$header" | tr -dc '0-9')"
  IFS= read -r blank || exit 0
  body="$(dd bs=1 count="$len" 2>/dev/null)"
  id="$(printf '%s' "$body" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')"
  case "$body" in
    *'"method":"initialize"'*)
      send "{\"jsonrpc\":\"2.0\",\"id\":${id:-1},\"result\":{\"capabilities\":{}}}"
      ;;
    *'"method":"textDocument/definition"'*)
      send "{\"jsonrpc\":\"2.0\",\"id\":${id:-1},\"result\":{\"uri\":\"file://$(pwd)/src/main.ts\",\"range\":{\"start\":{\"line\":0,\"character\":6},\"end\":{\"line\":0,\"character\":11}}}}"
      ;;
    *'"method":"textDocument/references"'*)
      send "{\"jsonrpc\":\"2.0\",\"id\":${id:-1},\"result\":[{\"uri\":\"file://$(pwd)/src/main.ts\",\"range\":{\"start\":{\"line\":0,\"character\":6},\"end\":{\"line\":0,\"character\":11}}}]}"
      ;;
    *'"id":'*)
      send "{\"jsonrpc\":\"2.0\",\"id\":${id:-1},\"result\":null}"
      ;;
  esac
done
"#;

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

    #[test]
    fn converts_one_based_byte_column_to_lsp_utf16_character() {
        let source = "fn main() {}\nlet name = \"value\";\nlet emoji = \"🚀\";\n";

        assert_eq!(
            utf16_character_for_one_based_byte_col(source, 2, 5),
            4,
            "ASCII column maps directly"
        );
        assert_eq!(
            utf16_character_for_one_based_byte_col(source, 3, 14),
            13,
            "columns before non-BMP text are unaffected"
        );
        assert_eq!(
            utf16_character_for_one_based_byte_col("let café_value = 1;\n", 1, 10),
            8,
            "multi-byte UTF-8 before the cursor is counted as UTF-16"
        );
    }

    #[test]
    fn workspace_configuration_response_matches_requested_item_count() {
        let response = workspace_configuration_result(&json!({
            "params": {
                "items": [{ "section": "rust-analyzer" }, { "section": "typescript" }]
            }
        }));
        assert_eq!(response, json!([{}, {}]));
    }

    #[test]
    fn javascript_language_id_distinguishes_jsx_files() {
        let spec = ServerSpec::for_language(SourceLanguage::JavaScript).unwrap();

        assert_eq!(
            spec.language_id_for_path(Path::new("src/app.ts")),
            "typescript"
        );
        assert_eq!(
            spec.language_id_for_path(Path::new("src/app.tsx")),
            "typescriptreact"
        );
        assert_eq!(
            spec.language_id_for_path(Path::new("src/app.js")),
            "javascript"
        );
        assert_eq!(
            spec.language_id_for_path(Path::new("src/app.jsx")),
            "javascriptreact"
        );
    }

    #[test]
    fn typescript_family_shares_one_server_key_per_root() {
        let root = CodeRootSpec {
            kind: super::super::indexer::CodeRootKind::Repo,
            name: "repo".to_string(),
            path: PathBuf::from("/repo"),
            repo_name: Some("repo".to_string()),
            workspace_id: None,
            git_head: None,
        };

        let ts_key = LspClientKey::new(
            &root,
            ServerSpec::for_language(SourceLanguage::TypeScript).unwrap(),
        );
        let tsx_key = LspClientKey::new(
            &root,
            ServerSpec::for_language(SourceLanguage::Tsx).unwrap(),
        );
        let js_key = LspClientKey::new(
            &root,
            ServerSpec::for_language(SourceLanguage::JavaScript).unwrap(),
        );
        let rust_key = LspClientKey::new(
            &root,
            ServerSpec::for_language(SourceLanguage::Rust).unwrap(),
        );

        assert_eq!(ts_key, tsx_key);
        assert_eq!(ts_key, js_key);
        assert_ne!(ts_key, rust_key);
    }

    #[tokio::test]
    async fn root_language_server_reuses_one_process_for_repeated_requests() {
        let temp = tempfile::tempdir().unwrap();
        let root_path = temp.path();
        std::fs::create_dir(root_path.join("src")).unwrap();
        let file_path = root_path.join("src/main.ts");
        let source = "const value = 1;\n";
        std::fs::write(&file_path, source).unwrap();
        let root = CodeRootSpec {
            kind: super::super::indexer::CodeRootKind::Repo,
            name: "repo".to_string(),
            path: root_path.to_path_buf(),
            repo_name: Some("repo".to_string()),
            workspace_id: None,
            git_head: None,
        };
        let spec = ServerSpec {
            language: "typescript",
            server_family: "typescript",
            source_languages: &[SourceLanguage::TypeScript],
            command: "/bin/sh",
            args: &["-c", FAKE_LSP_SCRIPT],
            required_commands: &[],
            language_id: "typescript",
        };
        let mut server = RootLanguageServer::spawn(spec, &root, Duration::from_secs(1))
            .await
            .unwrap();

        let first = server
            .request_locations(LspLocationRequest {
                file_path: &file_path,
                source,
                line: 1,
                col: 7,
                kind: LspRequestKind::Definition,
                request_timeout: Duration::from_secs(1),
                warmup_timeout: Duration::from_secs(1),
            })
            .await
            .unwrap();
        let second = server
            .request_locations(LspLocationRequest {
                file_path: &file_path,
                source,
                line: 1,
                col: 7,
                kind: LspRequestKind::Definition,
                request_timeout: Duration::from_secs(1),
                warmup_timeout: Duration::from_secs(1),
            })
            .await
            .unwrap();

        assert_eq!(first, second);
        assert_eq!(first[0].path, "src/main.ts");
        assert_eq!(
            std::fs::read_to_string(root_path.join(".fake_lsp_starts"))
                .unwrap()
                .trim(),
            "1"
        );
    }
}
