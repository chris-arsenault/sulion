use super::*;

#[derive(Debug, Deserialize)]
pub(in crate::retrieval) struct SearchQuery {
    pub(super) q: String,
    pub(super) scope: Option<String>,
    pub(super) repo: Option<String>,
    pub(super) agent_session_uuid: Option<Uuid>,
    pub(super) pty_session_id: Option<Uuid>,
    pub(super) workspace_id: Option<Uuid>,
    pub(super) cwd: Option<String>,
    pub(super) include: Option<String>,
    pub(super) file_path: Option<String>,
    pub(super) tool_category: Option<String>,
    pub(super) tool_name: Option<String>,
    pub(super) include_low_value: Option<bool>,
    pub(super) agent: Option<String>,
    pub(super) model: Option<String>,
    pub(super) errors_only: Option<bool>,
    pub(super) since: Option<DateTime<Utc>>,
    pub(super) until: Option<DateTime<Utc>>,
    pub(super) limit: Option<i64>,
    pub(super) search_mode: Option<String>,
}

#[derive(Debug, Serialize)]
pub(in crate::retrieval) struct SearchResponse {
    pub(super) context: ResolvedContext,
    pub(super) search_mode: String,
    pub(super) warnings: Vec<String>,
    pub(super) results: Vec<SearchResult>,
}

#[derive(Debug, Clone, Serialize)]
pub(in crate::retrieval) struct SearchResult {
    pub(super) source_kind: String,
    pub(super) score: f32,
    pub(super) lexical_score: Option<f32>,
    pub(super) semantic_score: Option<f32>,
    pub(super) repo: Option<String>,
    pub(super) agent_session_uuid: Uuid,
    pub(super) agent: String,
    pub(super) pty_session_id: Option<Uuid>,
    pub(super) turn_id: Option<i64>,
    pub(super) operation_ord: Option<i32>,
    pub(super) byte_offset: Option<i64>,
    pub(super) block_ord: Option<i32>,
    pub(super) timestamp: Option<DateTime<Utc>>,
    pub(super) preview: String,
    pub(super) snippet: String,
    pub(super) tool: Option<ToolSearchPayload>,
    pub(super) evidence: Option<EvidencePacket>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ToolSearchPayload {
    pub(super) name: String,
    pub(super) raw_name: Option<String>,
    pub(super) operation_type: Option<String>,
    pub(super) operation_category: Option<String>,
    pub(super) input: Option<Value>,
    pub(super) result_content: Option<String>,
    pub(super) result_payload: Option<Value>,
    pub(super) is_error: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(in crate::retrieval) struct EvidencePacket {
    pub(super) turn_preview: Option<String>,
    pub(super) turn_start_timestamp: Option<DateTime<Utc>>,
    pub(super) turn_end_timestamp: Option<DateTime<Utc>>,
    pub(super) operations: Vec<EvidenceOperation>,
    pub(super) file_touches: Vec<EvidenceFileTouch>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct EvidenceOperation {
    pub(super) name: String,
    pub(super) operation_category: Option<String>,
    pub(super) operation_type: Option<String>,
    pub(super) is_error: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct EvidenceFileTouch {
    pub(super) repo: String,
    pub(super) path: String,
    pub(super) touch_kind: String,
    pub(super) is_write: bool,
}

#[derive(Clone)]
pub(super) struct SearchFilters {
    pub(super) context: ResolvedContext,
    pub(super) include: HashSet<SourceKind>,
    pub(super) file_path: Option<String>,
    pub(super) tool_category: Option<String>,
    pub(super) tool_name: Option<String>,
    pub(super) include_low_value: bool,
    pub(super) agent: Option<String>,
    pub(super) model: Option<String>,
    pub(super) errors_only: bool,
    pub(super) since: Option<DateTime<Utc>>,
    pub(super) until: Option<DateTime<Utc>>,
    pub(super) limit: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct ResultKey {
    source_kind: String,
    session_uuid: Uuid,
    byte_offset: Option<i64>,
    block_ord: Option<i32>,
    turn_id: Option<i64>,
    operation_ord: Option<i32>,
}

impl ResultKey {
    pub(super) fn from_result(result: &SearchResult) -> Self {
        Self {
            source_kind: result.source_kind.clone(),
            session_uuid: result.agent_session_uuid,
            byte_offset: result.byte_offset,
            block_ord: result.block_ord,
            turn_id: result.turn_id,
            operation_ord: result.operation_ord,
        }
    }
}
