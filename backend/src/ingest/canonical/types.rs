use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::tools::canonicalize_tool_use;

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

/// App-facing semantic grouping for tool calls. This is intentionally
/// coarser than canonical tool names: the UI can facet on "research"
/// or "create_content" without baking agent/tool-specific semantics
/// into every view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationCategory {
    CreateContent,
    Inspect,
    Utility,
    Research,
    Delegate,
    Workflow,
    Other,
}

impl OperationCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            OperationCategory::CreateContent => "create_content",
            OperationCategory::Inspect => "inspect",
            OperationCategory::Utility => "utility",
            OperationCategory::Research => "research",
            OperationCategory::Delegate => "delegate",
            OperationCategory::Workflow => "workflow",
            OperationCategory::Other => "other",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        raw.parse().ok()
    }
}

impl std::str::FromStr for OperationCategory {
    type Err = ();

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "create_content" => Ok(OperationCategory::CreateContent),
            "inspect" => Ok(OperationCategory::Inspect),
            "utility" => Ok(OperationCategory::Utility),
            "research" => Ok(OperationCategory::Research),
            "delegate" => Ok(OperationCategory::Delegate),
            "workflow" => Ok(OperationCategory::Workflow),
            "other" => Ok(OperationCategory::Other),
            _ => Err(()),
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
    /// App-facing fine-grained action derived from ref data at read or
    /// projection time. This is intentionally not parser-owned raw
    /// transcript data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_type: Option<String>,
    /// App-facing semantic facet projected by the API from the
    /// `tool_category_rules` ref-data table. Parsers leave this empty;
    /// consumers should not treat it as raw transcript data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_category: Option<OperationCategory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_output: Option<Value>,
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
            operation_type: None,
            operation_category: None,
            tool_input: None,
            tool_output: None,
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
            operation_type: None,
            operation_category: None,
            tool_input: None,
            tool_output: None,
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
        let (canonical_name, canonical_input) = canonicalize_tool_use(&name, input);
        Self {
            ord,
            kind: BlockKind::ToolUse,
            text: None,
            tool_id: Some(tool_id.into()),
            tool_name_canonical: Some(canonical_name.clone()),
            tool_name: Some(name),
            operation_type: None,
            operation_category: None,
            tool_input: Some(canonical_input),
            tool_output: None,
            is_error: None,
            raw: None,
        }
    }

    pub fn tool_result(
        ord: i32,
        tool_id: impl Into<String>,
        text: Option<String>,
        is_error: bool,
        tool_output: Option<Value>,
    ) -> Self {
        Self {
            ord,
            kind: BlockKind::ToolResult,
            text,
            tool_id: Some(tool_id.into()),
            tool_name: None,
            tool_name_canonical: None,
            operation_type: None,
            operation_category: None,
            tool_input: None,
            tool_output,
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
            operation_type: None,
            operation_category: None,
            tool_input: None,
            tool_output: None,
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
                    if let Some(output) = &block.tool_output {
                        let serialized = output.to_string();
                        if serialized != "null" && !serialized.trim().is_empty() {
                            parts.push(serialized);
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

pub(crate) fn content_kind_of(blocks: &[Block]) -> ContentKind {
    if blocks.is_empty() {
        return ContentKind::None;
    }
    let mut has_text = false;
    let mut has_thinking = false;
    let mut has_tool_use = false;
    let mut has_tool_result = false;
    for block in blocks {
        match block.kind {
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
