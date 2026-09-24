//! App-shaped timeline projection.
//!
//! History rows are still the canonical, agent-agnostic transcript
//! surface. The timeline module turns those low-level events into product
//! concepts: turns, paired operations, visible items, previews, and
//! references to subagent runs.

mod events;
mod file_touches;
mod load;
pub(crate) mod reduce;
mod render;
mod runtime;
#[cfg(test)]
mod tests;
mod types;

pub use file_touches::{extract_file_touches, FileTouchContext};
pub use load::{count_session_events, load_session_events, resolve_session_target};
pub use types::{
    ProjectionFilters, ResolvedSession, SessionEventFilter, SessionLookup, SpeakerFacet,
    StoredEvent, TimelineAssistantItem, TimelineChunk, TimelineFileTouch, TimelineGenericDetails,
    TimelineItem, TimelineOperationBadge, TimelineResponse, TimelineSubagent,
    TimelineSummaryResponse, TimelineToolPair, TimelineToolResult, TimelineTurn,
    TimelineTurnDetailResponse, TimelineTurnSummary,
};

pub(crate) use render::{compose_turn_markdown, subagent_title};
pub(crate) use types::{is_bookkeeping_system_subtype, is_local_command_text, BOOKKEEPING_KINDS};
