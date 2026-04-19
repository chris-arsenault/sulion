//! Canonical event model — agent-agnostic structured representation of
//! a Claude Code / Codex / custom-agent JSONL record.
//!
//! The goal: the database is the integration point. REST handlers and
//! frontend renderers read canonical blocks + canonical tool names and
//! never inspect the raw `payload.message.content` shape. When Claude
//! changes its JSONL format, or a new agent gets plugged in, we only
//! touch one parser file; everything downstream is stable.
//!
//! Raw `events.payload` stays in the DB alongside the canonical blocks
//! as a forensic fallback and as the source for re-derivation should
//! the block shape evolve.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Speaker {
    User,
    Assistant,
    System,
    Summary,
    Other,
}

impl Speaker {
    pub fn as_str(&self) -> &'static str {
        match self {
            Speaker::User => "user",
            Speaker::Assistant => "assistant",
            Speaker::System => "system",
            Speaker::Summary => "summary",
            Speaker::Other => "other",
        }
    }
}

/// Coarse discriminator for the kinds of content an event carries.
/// `Mixed` means multiple kinds are present; use the block list to
/// distinguish. Good enough for quick filtering without a join.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    /// Event carries no blocks at all (bookkeeping, system init).
    None,
    /// Only text blocks.
    Text,
    /// Only thinking blocks (signature-only or full).
    Thinking,
    /// Contains at least one tool_use.
    ToolUse,
    /// Contains at least one tool_result.
    ToolResult,
    /// Mixed shape (common for assistant turns with text + tool_use).
    Mixed,
}

impl ContentKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ContentKind::None => "none",
            ContentKind::Text => "text",
            ContentKind::Thinking => "thinking",
            ContentKind::ToolUse => "tool_use",
            ContentKind::ToolResult => "tool_result",
            ContentKind::Mixed => "mixed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    Text,
    Thinking,
    ToolUse,
    ToolResult,
    /// Agent emitted a block shape the parser doesn't understand.
    /// `raw` carries the original JSON for backfill / re-derivation;
    /// the API intentionally hides it so consumers depend on the
    /// canonical surface instead.
    Unknown,
}

impl BlockKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            BlockKind::Text => "text",
            BlockKind::Thinking => "thinking",
            BlockKind::ToolUse => "tool_use",
            BlockKind::ToolResult => "tool_result",
            BlockKind::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub ord: i32,
    pub kind: BlockKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name_canonical: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

impl Block {
    pub fn text(ord: i32, text: impl Into<String>) -> Self {
        Self {
            ord,
            kind: BlockKind::Text,
            text: Some(text.into()),
            tool_id: None,
            tool_name: None,
            tool_name_canonical: None,
            tool_input: None,
            is_error: None,
            raw: None,
        }
    }

    pub fn thinking(ord: i32, text: impl Into<String>) -> Self {
        Self {
            ord,
            kind: BlockKind::Thinking,
            text: Some(text.into()),
            tool_id: None,
            tool_name: None,
            tool_name_canonical: None,
            tool_input: None,
            is_error: None,
            raw: None,
        }
    }

    pub fn tool_use(
        ord: i32,
        tool_id: impl Into<String>,
        tool_name: impl Into<String>,
        input: Value,
    ) -> Self {
        let name = tool_name.into();
        let canonical_name = canonicalize_tool_name(&name);
        Self {
            ord,
            kind: BlockKind::ToolUse,
            text: None,
            tool_id: Some(tool_id.into()),
            tool_name_canonical: Some(canonical_name.clone()),
            tool_name: Some(name),
            tool_input: Some(canonicalize_tool_input(&canonical_name, input)),
            is_error: None,
            raw: None,
        }
    }

    pub fn tool_result(
        ord: i32,
        tool_id: impl Into<String>,
        text: Option<String>,
        is_error: bool,
    ) -> Self {
        Self {
            ord,
            kind: BlockKind::ToolResult,
            text,
            tool_id: Some(tool_id.into()),
            tool_name: None,
            tool_name_canonical: None,
            tool_input: None,
            is_error: Some(is_error),
            raw: None,
        }
    }

    pub fn unknown(ord: i32, raw: Value) -> Self {
        Self {
            ord,
            kind: BlockKind::Unknown,
            text: None,
            tool_id: None,
            tool_name: None,
            tool_name_canonical: None,
            tool_input: None,
            is_error: None,
            raw: Some(raw),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalEvent {
    pub agent: &'static str,
    pub speaker: Speaker,
    pub content_kind: ContentKind,
    pub event_uuid: Option<String>,
    pub parent_event_uuid: Option<String>,
    pub related_tool_use_id: Option<String>,
    pub is_sidechain: bool,
    pub is_meta: bool,
    pub subtype: Option<String>,
    pub blocks: Vec<Block>,
}

impl CanonicalEvent {
    pub fn empty(agent: &'static str, speaker: Speaker) -> Self {
        Self {
            agent,
            speaker,
            content_kind: ContentKind::None,
            event_uuid: None,
            parent_event_uuid: None,
            related_tool_use_id: None,
            is_sidechain: false,
            is_meta: false,
            subtype: None,
            blocks: Vec::new(),
        }
    }

    pub fn search_text(&self) -> String {
        let mut parts = vec![self.speaker.as_str().to_string()];
        if let Some(subtype) = &self.subtype {
            if !subtype.trim().is_empty() {
                parts.push(subtype.clone());
            }
        }
        for block in &self.blocks {
            match block.kind {
                BlockKind::Text | BlockKind::Thinking | BlockKind::ToolResult => {
                    if let Some(text) = &block.text {
                        if !text.trim().is_empty() {
                            parts.push(text.clone());
                        }
                    }
                }
                BlockKind::ToolUse => {
                    if let Some(name) = &block.tool_name_canonical {
                        parts.push(name.clone());
                    }
                    if let Some(name) = &block.tool_name {
                        if !name.trim().is_empty() {
                            parts.push(name.clone());
                        }
                    }
                    if let Some(input) = &block.tool_input {
                        let serialized = input.to_string();
                        if serialized != "null" && !serialized.trim().is_empty() {
                            parts.push(serialized);
                        }
                    }
                }
                BlockKind::Unknown => {
                    if let Some(raw) = &block.raw {
                        let serialized = raw.to_string();
                        if !serialized.trim().is_empty() {
                            parts.push(serialized);
                        }
                    }
                }
            }
        }
        parts.join("\n")
    }
}

// ─── Tool name canonicalisation ──────────────────────────────────────
//
// Canonical names are snake_case verbs. Agent-specific variants map
// into them; unknown tools fall through to a lower-cased form of the
// raw name so callers still see *something* useful.

pub fn canonicalize_tool_name(raw: &str) -> String {
    match raw {
        // Claude Code builtins
        "Read" | "read_file" => "read",
        "Write" | "write_file" => "write",
        "Edit" | "apply_diff" => "edit",
        "MultiEdit" => "multi_edit",
        "Bash" | "shell" | "execute_shell" => "bash",
        "Grep" | "grep_search" => "grep",
        "Glob" | "glob_files" => "glob",
        "Task" | "spawn_agent" => "task",
        "TodoWrite" | "todo_update" => "todo_write",
        "WebFetch" | "fetch_url" => "web_fetch",
        "WebSearch" | "web_search_query" => "web_search",
        "NotebookEdit" => "notebook_edit",
        "BashOutput" => "bash_output",
        "KillShell" | "killShell" => "kill_shell",
        "ExitPlanMode" | "exitPlanMode" => "exit_plan_mode",
        other => {
            // Fallback: lowercase + snake_case. Keeps unknown agents
            // pointing at something consistent.
            return to_snake_case(other);
        }
    }
    .to_string()
}

fn to_snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let mut prev_lower = false;
    for c in s.chars() {
        if c.is_ascii_uppercase() {
            if prev_lower {
                out.push('_');
            }
            for lc in c.to_lowercase() {
                out.push(lc);
            }
            prev_lower = false;
        } else if c == '-' || c == ' ' {
            out.push('_');
            prev_lower = false;
        } else {
            out.push(c);
            prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        }
    }
    out
}

fn canonicalize_tool_input(canonical_name: &str, input: Value) -> Value {
    let Value::Object(obj) = input else {
        return input;
    };
    let mut out = Map::new();
    match canonical_name {
        "read" => {
            copy_first(&obj, &mut out, "path", &["path", "file_path"]);
            copy_key(&obj, &mut out, "offset");
            copy_key(&obj, &mut out, "limit");
        }
        "write" => {
            copy_first(&obj, &mut out, "path", &["path", "file_path"]);
            copy_key(&obj, &mut out, "content");
        }
        "edit" => {
            copy_first(&obj, &mut out, "path", &["path", "file_path"]);
            copy_first(&obj, &mut out, "old_text", &["old_text", "old_string"]);
            copy_first(&obj, &mut out, "new_text", &["new_text", "new_string"]);
            copy_key(&obj, &mut out, "replace_all");
        }
        "multi_edit" => {
            copy_first(&obj, &mut out, "path", &["path", "file_path"]);
            if let Some(Value::Array(edits)) = obj.get("edits") {
                let normalized = edits
                    .iter()
                    .map(|edit| normalize_edit(edit.clone()))
                    .collect::<Vec<_>>();
                out.insert("edits".to_string(), Value::Array(normalized));
            }
        }
        "bash" => {
            copy_key(&obj, &mut out, "command");
            copy_key(&obj, &mut out, "description");
        }
        "grep" => {
            copy_key(&obj, &mut out, "pattern");
            copy_first(&obj, &mut out, "path", &["path", "glob"]);
            copy_first(&obj, &mut out, "mode", &["mode", "output_mode"]);
        }
        "glob" => {
            copy_key(&obj, &mut out, "pattern");
            copy_key(&obj, &mut out, "path");
        }
        "task" => {
            copy_first(&obj, &mut out, "agent", &["agent", "subagent_type"]);
            copy_key(&obj, &mut out, "description");
            copy_key(&obj, &mut out, "prompt");
        }
        "todo_write" => {
            copy_key(&obj, &mut out, "todos");
        }
        "web_fetch" => {
            copy_key(&obj, &mut out, "url");
            copy_key(&obj, &mut out, "prompt");
        }
        "web_search" => {
            copy_key(&obj, &mut out, "query");
            copy_key(&obj, &mut out, "prompt");
        }
        _ => {
            return Value::Object(obj);
        }
    }
    Value::Object(out)
}

fn normalize_edit(edit: Value) -> Value {
    let Value::Object(obj) = edit else {
        return edit;
    };
    let mut out = Map::new();
    copy_first(&obj, &mut out, "old_text", &["old_text", "old_string"]);
    copy_first(&obj, &mut out, "new_text", &["new_text", "new_string"]);
    copy_key(&obj, &mut out, "replace_all");
    Value::Object(out)
}

fn copy_first(
    src: &Map<String, Value>,
    dst: &mut Map<String, Value>,
    target: &str,
    candidates: &[&str],
) {
    for key in candidates {
        if let Some(value) = src.get(*key) {
            dst.insert(target.to_string(), value.clone());
            return;
        }
    }
}

fn copy_key(src: &Map<String, Value>, dst: &mut Map<String, Value>, key: &str) {
    if let Some(value) = src.get(key) {
        dst.insert(key.to_string(), value.clone());
    }
}

// ─── Parser trait ────────────────────────────────────────────────────

pub trait EventParser: Send + Sync {
    fn agent_id(&self) -> &'static str;
    fn parse(&self, value: &Value) -> CanonicalEvent;
}

// ─── Claude Code parser (current shape) ──────────────────────────────

pub struct ClaudeParser;

impl EventParser for ClaudeParser {
    fn agent_id(&self) -> &'static str {
        "claude-code"
    }

    fn parse(&self, value: &Value) -> CanonicalEvent {
        let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let speaker = match kind {
            "user" => Speaker::User,
            "assistant" => Speaker::Assistant,
            "system" => Speaker::System,
            "summary" => Speaker::Summary,
            _ => Speaker::Other,
        };

        // Most content lives under `message.content`. When `content` is
        // a string rather than an array (happens on some user events),
        // treat the whole string as one text block.
        let content = value.get("message").and_then(|m| m.get("content"));

        let mut blocks = match content {
            Some(Value::Array(arr)) => parse_blocks(arr),
            Some(Value::String(s)) => vec![Block::text(0, s.clone())],
            _ => Vec::new(),
        };
        if blocks.is_empty() && kind == "summary" {
            if let Some(summary) = value.get("summary").and_then(|v| v.as_str()) {
                blocks.push(Block::text(0, summary.to_string()));
            }
        }

        let content_kind = content_kind_of(&blocks);

        CanonicalEvent {
            agent: self.agent_id(),
            speaker,
            content_kind,
            event_uuid: string_field(value, &["uuid"]),
            parent_event_uuid: string_field(value, &["parentUuid", "parent_uuid"]),
            related_tool_use_id: string_field(value, &["tool_use_id"]),
            is_sidechain: bool_field(value, &["isSidechain"]).unwrap_or(false),
            is_meta: bool_field(value, &["isMeta"]).unwrap_or(false),
            subtype: string_field(value, &["subtype"]),
            blocks,
        }
    }
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(|v| v.as_str()))
        .map(ToString::to_string)
}

fn bool_field(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(|v| v.as_bool()))
}

fn parse_blocks(arr: &[Value]) -> Vec<Block> {
    let mut out = Vec::with_capacity(arr.len());
    for (i, raw) in arr.iter().enumerate() {
        let ord = i as i32;
        let Some(ty) = raw.get("type").and_then(|v| v.as_str()) else {
            out.push(Block::unknown(ord, raw.clone()));
            continue;
        };
        match ty {
            "text" => {
                let text = raw
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                out.push(Block::text(ord, text));
            }
            "thinking" => {
                let text = raw
                    .get("thinking")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                out.push(Block::thinking(ord, text));
            }
            "tool_use" => {
                let id = raw
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = raw
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let input = raw.get("input").cloned().unwrap_or(Value::Null);
                out.push(Block::tool_use(ord, id, name, input));
            }
            "tool_result" => {
                let id = raw
                    .get("tool_use_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let is_error = raw
                    .get("is_error")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                // `content` here can be a string, an array of {type:text, text}
                // blocks, or an object — flatten to a string.
                let text = match raw.get("content") {
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(Value::Array(parts)) => {
                        let mut buf = String::new();
                        for p in parts {
                            if let Some(s) = p.get("text").and_then(|v| v.as_str()) {
                                if !buf.is_empty() {
                                    buf.push('\n');
                                }
                                buf.push_str(s);
                            }
                        }
                        if buf.is_empty() {
                            None
                        } else {
                            Some(buf)
                        }
                    }
                    Some(other) => Some(other.to_string()),
                    None => None,
                };
                out.push(Block::tool_result(ord, id, text, is_error));
            }
            _ => {
                out.push(Block::unknown(ord, raw.clone()));
            }
        }
    }
    out
}

fn content_kind_of(blocks: &[Block]) -> ContentKind {
    if blocks.is_empty() {
        return ContentKind::None;
    }
    let mut has_text = false;
    let mut has_thinking = false;
    let mut has_tool_use = false;
    let mut has_tool_result = false;
    for b in blocks {
        match b.kind {
            BlockKind::Text => has_text = true,
            BlockKind::Thinking => has_thinking = true,
            BlockKind::ToolUse => has_tool_use = true,
            BlockKind::ToolResult => has_tool_result = true,
            BlockKind::Unknown => {}
        }
    }
    let count =
        (has_text as u8) + (has_thinking as u8) + (has_tool_use as u8) + (has_tool_result as u8);
    match (count, has_text, has_thinking, has_tool_use, has_tool_result) {
        (0, _, _, _, _) => ContentKind::None,
        (1, true, _, _, _) => ContentKind::Text,
        (1, _, true, _, _) => ContentKind::Thinking,
        (1, _, _, true, _) => ContentKind::ToolUse,
        (1, _, _, _, true) => ContentKind::ToolResult,
        _ => ContentKind::Mixed,
    }
}

// ─── Codex parser ────────────────────────────────────────────────────
//
// Codex transcripts are higher-level than Claude's JSONL: the durable
// conversational artefacts live under `response_item`, while `event_msg`
// and `turn_context` mostly carry telemetry/UI state. We canonicalise
// the former into messages/thinking/tool use/tool result blocks and mark
// the latter as bookkeeping.

pub struct CodexParser;

impl EventParser for CodexParser {
    fn agent_id(&self) -> &'static str {
        "codex"
    }

    fn parse(&self, value: &Value) -> CanonicalEvent {
        let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "response_item" => parse_codex_response_item(value),
            "event_msg" => parse_codex_event_msg(value),
            "session_meta" | "turn_context" | "compacted" => CanonicalEvent {
                agent: self.agent_id(),
                speaker: Speaker::System,
                content_kind: ContentKind::None,
                event_uuid: None,
                parent_event_uuid: None,
                related_tool_use_id: None,
                is_sidechain: false,
                is_meta: true,
                subtype: Some(kind.to_string()),
                blocks: Vec::new(),
            },
            _ => CanonicalEvent {
                agent: self.agent_id(),
                speaker: Speaker::Other,
                content_kind: ContentKind::None,
                event_uuid: None,
                parent_event_uuid: None,
                related_tool_use_id: None,
                is_sidechain: false,
                is_meta: true,
                subtype: Some(kind.to_string()),
                blocks: vec![Block::unknown(0, value.clone())],
            },
        }
    }
}

fn parse_codex_response_item(value: &Value) -> CanonicalEvent {
    let payload = value.get("payload").unwrap_or(&Value::Null);
    let subtype = payload
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("response_item");
    match subtype {
        "message" => {
            let role = payload
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("other");
            let speaker = match role {
                "user" => Speaker::User,
                "assistant" => Speaker::Assistant,
                "developer" | "system" => Speaker::System,
                _ => Speaker::Other,
            };
            let is_meta = matches!(role, "developer" | "system");
            let blocks = match payload.get("content") {
                Some(Value::Array(items)) => parse_codex_message_items(items),
                Some(Value::String(s)) => vec![Block::text(0, s.clone())],
                _ => Vec::new(),
            };
            CanonicalEvent {
                agent: "codex",
                speaker,
                content_kind: content_kind_of(&blocks),
                event_uuid: None,
                parent_event_uuid: None,
                related_tool_use_id: None,
                is_sidechain: false,
                is_meta,
                subtype: Some(subtype.to_string()),
                blocks,
            }
        }
        "reasoning" => {
            let text = codex_reasoning_text(payload).unwrap_or_default();
            let blocks = vec![Block::thinking(0, text)];
            CanonicalEvent {
                agent: "codex",
                speaker: Speaker::Assistant,
                content_kind: content_kind_of(&blocks),
                event_uuid: None,
                parent_event_uuid: None,
                related_tool_use_id: None,
                is_sidechain: false,
                is_meta: false,
                subtype: Some(subtype.to_string()),
                blocks,
            }
        }
        "function_call" | "custom_tool_call" => {
            let call_id = payload
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = payload
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(subtype)
                .to_string();
            let input = if subtype == "function_call" {
                parse_json_string(payload.get("arguments"))
            } else {
                payload.get("input").cloned().unwrap_or(Value::Null)
            };
            let blocks = vec![Block::tool_use(0, call_id, name, input)];
            CanonicalEvent {
                agent: "codex",
                speaker: Speaker::Assistant,
                content_kind: content_kind_of(&blocks),
                event_uuid: None,
                parent_event_uuid: None,
                related_tool_use_id: None,
                is_sidechain: false,
                is_meta: false,
                subtype: Some(subtype.to_string()),
                blocks,
            }
        }
        "function_call_output" | "custom_tool_call_output" => {
            let call_id = payload
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let text = payload.get("output").and_then(value_to_text);
            let is_error = payload
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let blocks = vec![Block::tool_result(0, call_id.clone(), text, is_error)];
            CanonicalEvent {
                agent: "codex",
                speaker: Speaker::System,
                content_kind: content_kind_of(&blocks),
                event_uuid: None,
                parent_event_uuid: None,
                related_tool_use_id: Some(call_id),
                is_sidechain: false,
                is_meta: false,
                subtype: Some(subtype.to_string()),
                blocks,
            }
        }
        _ => CanonicalEvent {
            agent: "codex",
            speaker: Speaker::System,
            content_kind: ContentKind::None,
            event_uuid: None,
            parent_event_uuid: None,
            related_tool_use_id: None,
            is_sidechain: false,
            is_meta: true,
            subtype: Some(subtype.to_string()),
            blocks: Vec::new(),
        },
    }
}

fn parse_codex_event_msg(value: &Value) -> CanonicalEvent {
    let payload = value.get("payload").unwrap_or(&Value::Null);
    let subtype = payload
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("event_msg");
    CanonicalEvent {
        agent: "codex",
        speaker: Speaker::System,
        content_kind: ContentKind::None,
        event_uuid: None,
        parent_event_uuid: None,
        related_tool_use_id: payload
            .get("call_id")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        is_sidechain: false,
        is_meta: true,
        subtype: Some(subtype.to_string()),
        blocks: Vec::new(),
    }
}

fn parse_codex_message_items(items: &[Value]) -> Vec<Block> {
    let mut blocks = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let ord = i as i32;
        let ty = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match ty {
            "input_text" | "output_text" => {
                let text = item
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                blocks.push(Block::text(ord, text));
            }
            _ => blocks.push(Block::unknown(ord, item.clone())),
        }
    }
    blocks
}

fn codex_reasoning_text(payload: &Value) -> Option<String> {
    let summary = payload.get("summary")?.as_array()?;
    let mut parts = Vec::new();
    for item in summary {
        if let Some(s) = item.as_str() {
            if !s.trim().is_empty() {
                parts.push(s.to_string());
            }
            continue;
        }
        if let Some(s) = item.get("text").and_then(|v| v.as_str()) {
            if !s.trim().is_empty() {
                parts.push(s.to_string());
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn parse_json_string(value: Option<&Value>) -> Value {
    let Some(Value::String(raw)) = value else {
        return value.cloned().unwrap_or(Value::Null);
    };
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.clone()))
}

fn value_to_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => {
            let mut out = Vec::new();
            for part in parts {
                if let Some(text) = value_to_text(part) {
                    if !text.trim().is_empty() {
                        out.push(text);
                    }
                }
            }
            if out.is_empty() {
                None
            } else {
                Some(out.join("\n"))
            }
        }
        Value::Object(obj) => obj
            .get("text")
            .and_then(|v| v.as_str())
            .map(ToString::to_string)
            .or_else(|| Some(Value::Object(obj.clone()).to_string())),
        other => Some(other.to_string()),
    }
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse_claude(v: Value) -> CanonicalEvent {
        ClaudeParser.parse(&v)
    }

    fn parse_codex(v: Value) -> CanonicalEvent {
        CodexParser.parse(&v)
    }

    #[test]
    fn tool_names_canonicalise() {
        assert_eq!(canonicalize_tool_name("Read"), "read");
        assert_eq!(canonicalize_tool_name("Edit"), "edit");
        assert_eq!(canonicalize_tool_name("MultiEdit"), "multi_edit");
        assert_eq!(canonicalize_tool_name("read_file"), "read");
        assert_eq!(canonicalize_tool_name("apply_patch"), "apply_patch");
        assert_eq!(canonicalize_tool_name("Bash"), "bash");
        assert_eq!(canonicalize_tool_name("WebFetch"), "web_fetch");
        // Unknown → snake_case
        assert_eq!(canonicalize_tool_name("SomeNewThing"), "some_new_thing");
        assert_eq!(canonicalize_tool_name("custom-tool"), "custom_tool");
        assert_eq!(canonicalize_tool_name("already_snake"), "already_snake");
    }

    #[test]
    fn claude_user_text_string_content() {
        let ev = parse_claude(json!({
            "type": "user",
            "message": { "role": "user", "content": "hello world" }
        }));
        assert_eq!(ev.speaker, Speaker::User);
        assert_eq!(ev.content_kind, ContentKind::Text);
        assert_eq!(ev.blocks.len(), 1);
        assert_eq!(ev.blocks[0].kind, BlockKind::Text);
        assert_eq!(ev.blocks[0].text.as_deref(), Some("hello world"));
    }

    #[test]
    fn claude_assistant_text_array() {
        let ev = parse_claude(json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                { "type": "text", "text": "first" },
                { "type": "text", "text": "second" }
            ]}
        }));
        assert_eq!(ev.speaker, Speaker::Assistant);
        assert_eq!(ev.content_kind, ContentKind::Text);
        assert_eq!(ev.blocks.len(), 2);
        assert_eq!(ev.blocks[0].text.as_deref(), Some("first"));
        assert_eq!(ev.blocks[1].text.as_deref(), Some("second"));
        assert_eq!(ev.blocks[1].ord, 1);
    }

    #[test]
    fn claude_tool_use_canonical_name_applied() {
        let ev = parse_claude(json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                { "type": "tool_use", "id": "t1", "name": "Read",
                  "input": { "file_path": "/a/b.rs" }}
            ]}
        }));
        assert_eq!(ev.content_kind, ContentKind::ToolUse);
        let b = &ev.blocks[0];
        assert_eq!(b.kind, BlockKind::ToolUse);
        assert_eq!(b.tool_id.as_deref(), Some("t1"));
        assert_eq!(b.tool_name.as_deref(), Some("Read"));
        assert_eq!(b.tool_name_canonical.as_deref(), Some("read"));
        assert_eq!(
            b.tool_input
                .as_ref()
                .and_then(|v| v.get("path"))
                .and_then(|v| v.as_str()),
            Some("/a/b.rs")
        );
    }

    #[test]
    fn claude_event_metadata_is_canonicalised() {
        let ev = parse_claude(json!({
            "type": "system",
            "uuid": "ev-1",
            "parentUuid": "ev-0",
            "tool_use_id": "task-1",
            "isSidechain": true,
            "isMeta": true,
            "subtype": "permission-mode"
        }));
        assert_eq!(ev.event_uuid.as_deref(), Some("ev-1"));
        assert_eq!(ev.parent_event_uuid.as_deref(), Some("ev-0"));
        assert_eq!(ev.related_tool_use_id.as_deref(), Some("task-1"));
        assert!(ev.is_sidechain);
        assert!(ev.is_meta);
        assert_eq!(ev.subtype.as_deref(), Some("permission-mode"));
    }

    #[test]
    fn claude_tool_result_is_error_and_text_variants() {
        // string content
        let ev = parse_claude(json!({
            "type": "user",
            "message": { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "t1", "content": "out",
                  "is_error": false }
            ]}
        }));
        assert_eq!(ev.content_kind, ContentKind::ToolResult);
        assert_eq!(ev.blocks[0].kind, BlockKind::ToolResult);
        assert_eq!(ev.blocks[0].text.as_deref(), Some("out"));
        assert_eq!(ev.blocks[0].is_error, Some(false));

        // array content
        let ev = parse_claude(json!({
            "type": "user",
            "message": { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "t2",
                  "content": [
                      { "type": "text", "text": "a" },
                      { "type": "text", "text": "b" }
                  ],
                  "is_error": true }
            ]}
        }));
        assert_eq!(ev.blocks[0].text.as_deref(), Some("a\nb"));
        assert_eq!(ev.blocks[0].is_error, Some(true));
    }

    #[test]
    fn claude_mixed_content_reports_mixed() {
        let ev = parse_claude(json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                { "type": "text", "text": "thinking through" },
                { "type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}},
            ]}
        }));
        assert_eq!(ev.content_kind, ContentKind::Mixed);
        assert_eq!(ev.blocks.len(), 2);
    }

    #[test]
    fn claude_unknown_block_preserved() {
        let ev = parse_claude(json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                { "type": "hypothetical_future_block", "payload": { "x": 1 } }
            ]}
        }));
        assert_eq!(ev.blocks.len(), 1);
        assert_eq!(ev.blocks[0].kind, BlockKind::Unknown);
        assert!(ev.blocks[0].raw.is_some());
    }

    #[test]
    fn claude_summary_event() {
        let ev = parse_claude(json!({
            "type": "summary",
            "summary": "compact summary text"
        }));
        assert_eq!(ev.speaker, Speaker::Summary);
        assert_eq!(ev.blocks.len(), 1);
        assert_eq!(ev.blocks[0].text.as_deref(), Some("compact summary text"));
        assert_eq!(ev.content_kind, ContentKind::Text);
    }

    #[test]
    fn search_text_uses_canonical_tool_input() {
        let ev = parse_claude(json!({
            "type": "assistant",
            "subtype": "tool-preview",
            "message": { "role": "assistant", "content": [
                { "type": "text", "text": "reading now" },
                { "type": "tool_use", "id": "t1", "name": "Read",
                  "input": { "file_path": "/tmp/a.txt", "offset": 10 }}
            ]}
        }));
        let search = ev.search_text();
        assert!(search.contains("tool-preview"));
        assert!(search.contains("reading now"));
        assert!(search.contains("read"));
        assert!(search.contains("/tmp/a.txt"));
        assert!(!search.contains("file_path"));
    }

    #[test]
    fn codex_message_maps_to_text_blocks() {
        let ev = parse_codex(json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": [
                    { "type": "output_text", "text": "a codex reply" }
                ]
            }
        }));
        assert_eq!(ev.agent, "codex");
        assert_eq!(ev.speaker, Speaker::Assistant);
        assert_eq!(ev.blocks.len(), 1);
        assert_eq!(ev.blocks[0].kind, BlockKind::Text);
        assert_eq!(ev.blocks[0].text.as_deref(), Some("a codex reply"));
    }

    #[test]
    fn codex_function_call_and_output_map_to_tool_blocks() {
        let ev = parse_codex(json!({
            "type": "response_item",
            "payload": {
                "type": "function_call",
                "name": "exec_command",
                "call_id": "call-1",
                "arguments": "{\"cmd\":\"git status --short\",\"workdir\":\"/tmp/demo\"}"
            }
        }));
        assert_eq!(ev.speaker, Speaker::Assistant);
        assert_eq!(ev.content_kind, ContentKind::ToolUse);
        assert_eq!(ev.blocks[0].kind, BlockKind::ToolUse);
        assert_eq!(ev.blocks[0].tool_id.as_deref(), Some("call-1"));
        assert_eq!(
            ev.blocks[0].tool_name_canonical.as_deref(),
            Some("exec_command")
        );
        assert_eq!(
            ev.blocks[0]
                .tool_input
                .as_ref()
                .and_then(|v| v.get("cmd"))
                .and_then(|v| v.as_str()),
            Some("git status --short")
        );

        let ev = parse_codex(json!({
            "type": "response_item",
            "payload": {
                "type": "function_call_output",
                "call_id": "call-1",
                "output": "ok"
            }
        }));
        assert_eq!(ev.content_kind, ContentKind::ToolResult);
        assert_eq!(ev.related_tool_use_id.as_deref(), Some("call-1"));
        assert_eq!(ev.blocks[0].kind, BlockKind::ToolResult);
        assert_eq!(ev.blocks[0].tool_id.as_deref(), Some("call-1"));
        assert_eq!(ev.blocks[0].text.as_deref(), Some("ok"));
    }

    #[test]
    fn codex_reasoning_and_meta_records_are_preserved() {
        let ev = parse_codex(json!({
            "type": "response_item",
            "payload": {
                "type": "reasoning",
                "summary": []
            }
        }));
        assert_eq!(ev.speaker, Speaker::Assistant);
        assert_eq!(ev.content_kind, ContentKind::Thinking);
        assert_eq!(ev.blocks[0].kind, BlockKind::Thinking);

        let ev = parse_codex(json!({
            "type": "session_meta",
            "payload": { "id": "abc" }
        }));
        assert_eq!(ev.speaker, Speaker::System);
        assert!(ev.is_meta);
        assert_eq!(ev.subtype.as_deref(), Some("session_meta"));
        assert!(ev.blocks.is_empty());
    }

    #[test]
    fn empty_thinking_still_a_thinking_block() {
        // Claude emits signature-only thinking; the frontend filters
        // these out for chip rendering, but the block must still exist
        // so other consumers (counters, future export formats) can see
        // that thinking happened.
        let ev = parse_claude(json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                { "type": "thinking", "thinking": "" }
            ]}
        }));
        assert_eq!(ev.blocks[0].kind, BlockKind::Thinking);
        assert_eq!(ev.blocks[0].text.as_deref(), Some(""));
    }
}
