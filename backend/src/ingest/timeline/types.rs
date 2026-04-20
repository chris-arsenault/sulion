use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::ingest::canonical::{Block, OperationCategory};

pub(crate) const BOOKKEEPING_KINDS: &[&str] = &[
    "file-history-snapshot",
    "permission-mode",
    "last-prompt",
    "queue-operation",
    "attachment",
];

#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub byte_offset: i64,
    pub timestamp: DateTime<Utc>,
    pub kind: String,
    pub agent: String,
    pub speaker: Option<String>,
    pub content_kind: Option<String>,
    pub event_uuid: Option<String>,
    pub parent_event_uuid: Option<String>,
    pub related_tool_use_id: Option<String>,
    pub is_sidechain: bool,
    pub is_meta: bool,
    pub subtype: Option<String>,
    pub blocks: Vec<Block>,
}
#[derive(Debug, Clone)]
pub struct ResolvedSession {
    pub session_uuid: Uuid,
    pub session_agent: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SessionLookup {
    MissingPty,
    NoSession,
    Resolved(ResolvedSession),
}

#[derive(Debug, Clone, Default)]
pub struct SessionEventFilter {
    pub after: Option<i64>,
    pub limit: Option<i64>,
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpeakerFacet {
    User,
    Assistant,
    ToolResult,
}

#[derive(Debug, Clone, Default)]
pub struct ProjectionFilters {
    pub hidden_speakers: HashSet<SpeakerFacet>,
    pub hidden_operation_categories: HashSet<OperationCategory>,
    pub errors_only: bool,
    pub show_bookkeeping: bool,
    pub show_sidechain: bool,
    pub file_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineResponse {
    pub session_uuid: Option<Uuid>,
    pub session_agent: Option<String>,
    pub total_event_count: i64,
    pub turns: Vec<TimelineTurn>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineTurn {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_key: Option<String>,
    pub preview: String,
    pub user_prompt_text: Option<String>,
    pub start_timestamp: DateTime<Utc>,
    pub end_timestamp: DateTime<Utc>,
    pub duration_ms: i64,
    pub event_count: usize,
    pub operation_count: usize,
    pub tool_pairs: Vec<TimelineToolPair>,
    pub thinking_count: usize,
    pub has_errors: bool,
    pub markdown: String,
    pub chunks: Vec<TimelineChunk>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pty_session_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_uuid: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineToolPair {
    pub id: String,
    pub name: String,
    pub raw_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_type: Option<String>,
    pub category: Option<OperationCategory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<TimelineToolResult>,
    pub is_error: bool,
    pub is_pending: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_touches: Vec<TimelineFileTouch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent: Option<Box<TimelineSubagent>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TimelineFileTouch {
    pub repo: String,
    pub path: String,
    pub touch_kind: String,
    pub is_write: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineToolResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineSubagent {
    pub title: String,
    pub event_count: usize,
    pub turns: Vec<TimelineTurn>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TimelineChunk {
    Assistant {
        items: Vec<TimelineAssistantItem>,
        thinking: Vec<String>,
    },
    Tool {
        pair_id: String,
    },
    Summary {
        subtype: Option<String>,
        text: String,
    },
    System {
        subtype: Option<String>,
        text: String,
        is_meta: bool,
    },
    Generic {
        label: String,
        details: TimelineGenericDetails,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TimelineAssistantItem {
    Text { text: String },
    Tool { pair_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineGenericDetails {
    pub event_uuid: Option<String>,
    pub parent_event_uuid: Option<String>,
    pub related_tool_use_id: Option<String>,
    pub subtype: Option<String>,
    pub speaker: Option<String>,
    pub content_kind: Option<String>,
    pub blocks: Vec<Block>,
}
