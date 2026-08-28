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

mod claude;
mod code_mode;
mod codex;
#[cfg(test)]
mod tests;
mod tools;
mod types;

pub use claude::ClaudeParser;
pub use codex::{codex_record_kind, CodexParser};
pub use tools::canonicalize_tool_name;
pub use types::{Block, BlockKind, CanonicalEvent, ContentKind, OperationCategory, Speaker};

pub(crate) use tools::canonicalize_tool_result_payload;
pub(crate) use types::content_kind_of;

pub trait EventParser: Send + Sync {
    fn agent_id(&self) -> &'static str;
    fn parse(&self, value: &serde_json::Value) -> CanonicalEvent;
}
