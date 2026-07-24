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

mod protocol;
#[cfg(test)]
mod tests;

use protocol::*;

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
