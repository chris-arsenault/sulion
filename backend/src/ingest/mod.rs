mod activity_projection;
pub mod canonical;
mod file_scan;
mod ingester;
mod maintenance;
mod metadata;
mod projection;
mod reset;
mod tail;
pub mod timeline;
mod usage;

pub use ingester::{backfill_canonical_blocks, parse_codex_session_uuid, Ingester, IngesterConfig};
pub use maintenance::{
    mark_projection_versions_current, run_required_startup_maintenance, StartupMaintenanceStats,
};
pub use metadata::upsert_from_event as upsert_session_metadata_from_event;
pub use projection::{
    annotate_timeline_summaries, annotate_timeline_turns, backfill_timeline_projection,
    load_repo_file_trace, load_repo_timeline_summary_response, load_timeline_response,
    load_timeline_session_meta, load_timeline_summary_response, load_timeline_turn_detail,
    rebuild_session_projection, RepoFileTraceTouch, TimelineSessionMeta,
};
pub use reset::{rebuild_ingest_derivatives, ReindexStats};

/// What the read paths need, without reaching into the parse and projection
/// internals behind them. `canonical` is the ingester's JSONL representation
/// and `timeline` is its projection module; consumers want the block type and
/// the session-lookup entry point, not the modules those live in.
pub use canonical::{Block as CanonicalBlock, OperationCategory};
pub use timeline::{
    load_session_events, resolve_session_target, ProjectionFilters, SessionEventFilter,
    SessionLookup, SpeakerFacet, TimelineSummaryResponse, TimelineTurn,
    TimelineTurnDetailResponse,
};
