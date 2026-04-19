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
use serde_json::Value;

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
    /// `raw` carries the original JSON so the frontend can render a
    /// placeholder without data loss.
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
        Self {
            ord,
            kind: BlockKind::ToolUse,
            text: None,
            tool_id: Some(tool_id.into()),
            tool_name_canonical: Some(canonicalize_tool_name(&name)),
            tool_name: Some(name),
            tool_input: Some(input),
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
    pub blocks: Vec<Block>,
}

impl CanonicalEvent {
    pub fn empty(agent: &'static str, speaker: Speaker) -> Self {
        Self {
            agent,
            speaker,
            content_kind: ContentKind::None,
            blocks: Vec::new(),
        }
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
        "Edit" | "apply_patch" | "apply_diff" => "edit",
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

        let blocks = match content {
            Some(Value::Array(arr)) => parse_blocks(arr),
            Some(Value::String(s)) => vec![Block::text(0, s.clone())],
            _ => Vec::new(),
        };

        let content_kind = content_kind_of(&blocks);

        CanonicalEvent {
            agent: self.agent_id(),
            speaker,
            content_kind,
            blocks,
        }
    }
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

// ─── Codex parser (stub) ─────────────────────────────────────────────
//
// Placeholder to prove the parser extension point. Not wired into the
// ingester yet — we pick the parser statically based on the agent id
// for now, and the ingester only feeds Claude transcripts. When Codex
// (or the OpenAI Responses stream) actually lands, fill in `parse` with
// the canonical mapping; the downstream frontend won't change.

pub struct CodexParser;

impl EventParser for CodexParser {
    fn agent_id(&self) -> &'static str {
        "codex"
    }

    fn parse(&self, value: &Value) -> CanonicalEvent {
        // Minimal: if the record has a top-level "content" string, emit
        // a text block; otherwise emit a single unknown block so the
        // frontend still has something to render. Intentionally small.
        let speaker = match value.get("role").and_then(|v| v.as_str()) {
            Some("user") => Speaker::User,
            Some("assistant") => Speaker::Assistant,
            Some("system") => Speaker::System,
            _ => Speaker::Other,
        };
        let blocks = if let Some(s) = value.get("content").and_then(|v| v.as_str()) {
            vec![Block::text(0, s)]
        } else {
            vec![Block::unknown(0, value.clone())]
        };
        let content_kind = content_kind_of(&blocks);
        CanonicalEvent {
            agent: self.agent_id(),
            speaker,
            content_kind,
            blocks,
        }
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

    #[test]
    fn tool_names_canonicalise() {
        assert_eq!(canonicalize_tool_name("Read"), "read");
        assert_eq!(canonicalize_tool_name("Edit"), "edit");
        assert_eq!(canonicalize_tool_name("MultiEdit"), "multi_edit");
        assert_eq!(canonicalize_tool_name("read_file"), "read");
        assert_eq!(canonicalize_tool_name("apply_patch"), "edit");
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
                .and_then(|v| v.get("file_path"))
                .and_then(|v| v.as_str()),
            Some("/a/b.rs")
        );
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
        // No content array → no blocks.
        assert_eq!(ev.blocks.len(), 0);
        assert_eq!(ev.content_kind, ContentKind::None);
    }

    #[test]
    fn codex_stub_degrades_gracefully() {
        let ev = CodexParser.parse(&json!({
            "role": "assistant",
            "content": "a codex reply"
        }));
        assert_eq!(ev.agent, "codex");
        assert_eq!(ev.speaker, Speaker::Assistant);
        assert_eq!(ev.blocks.len(), 1);
        assert_eq!(ev.blocks[0].kind, BlockKind::Text);

        let ev = CodexParser.parse(&json!({ "some": "unrecognised" }));
        assert_eq!(ev.blocks.len(), 1);
        assert_eq!(ev.blocks[0].kind, BlockKind::Unknown);
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
