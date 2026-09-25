// Mirrors the Rust SessionView in backend/src/routes.rs. Keep in sync
// manually — a small enough surface that codegen would be overkill.

export type SessionState = "live" | "dead" | "deleted" | "orphaned";
export type AgentLaunchType = "claude" | "codex" | "fugu";
export type AgentRuntimeState = "none" | "starting" | "running" | "exited";
export type SessionActivityState =
  | "shell"
  | "starting"
  | "working"
  | "awaiting_prompt"
  | "needs_input"
  | "blocked"
  | "unknown";

export interface AgentRuntimeMetadata {
  agent: AgentLaunchType | string | null;
  state: AgentRuntimeState;
  started_at: string | null;
  ended_at: string | null;
  exit_code: number | null;
}

export interface AgentSessionMetadata {
  agent: string;
  model: string | null;
  model_provider: string | null;
  reasoning_effort: string | null;
  cli_version: string | null;
  cwd: string | null;
  model_context_window: number | null;
  updated_at: string;
}

export interface AgentSessionUsage {
  input_tokens: number;
  cached_input_tokens: number;
  cache_write_input_tokens: number;
  cache_write_1h_input_tokens: number;
  output_tokens: number;
  reasoning_output_tokens: number;
  total_tokens: number;
  /** Estimated token footprint of the latest model call. */
  context_tokens: number | null;
  model_context_window: number | null;
  observed_at: string;
  updated_at: string;
}

export interface SessionActivity {
  state: SessionActivityState;
  summary: string | null;
  reason: string | null;
  source: "launcher" | "hook" | "ingester" | "agent" | "user" | string;
  confidence: "explicit" | "derived" | "unknown" | string;
  updated_at: string | null;
}

export interface CurrentPlanView {
  id: string;
  title: string;
  status: PlanStatus;
  revision: number;
  /** 0 for a root plan; higher means this PTY is working a branch. */
  depth: number;
  /** Title of the tree's root, present only when depth > 0. */
  root_title: string | null;
  total_phases: number;
  completed_phases: number;
  current_phase_id: string | null;
  current_phase_title: string | null;
  current_phase_status: PlanPhaseStatus | null;
}

export interface SessionView {
  id: string;
  repo: string;
  working_dir: string;
  workspace?: SessionWorkspaceView | null;
  /** Collection identity when this PTY was launched for a meta-repository. */
  meta_repo?: SessionMetaRepoView | null;
  state: SessionState;
  created_at: string;
  ended_at: string | null;
  exit_code: number | null;
  current_session_uuid: string | null;
  current_session_agent: string | null;
  /** MAX(event.timestamp) for this session's current transcript session.
   * Null when no events have been ingested yet. Drives the sidebar
   * unread-dot indicator. */
  last_event_at: string | null;
  /** Backend-maintained revision for this transcript's projected timeline. */
  timeline_revision?: number;
  /** User-facing label; overrides the uuid prefix in the sidebar. */
  label: string | null;
  /** Agent-chosen terminal name (`sulion name`). Shown beside the
   * user's label in the sidebar and monitor; never in tab headers. */
  agent_label?: string | null;
  /** Pinned sessions float to the top of their repo group. */
  pinned: boolean;
  /** Palette-constrained colour tag name. */
  color: SessionColor | null;
  /** PTY-scoped first-class agent process state. */
  agent_runtime?: AgentRuntimeMetadata;
  /** Transcript-derived agent metadata for the currently correlated session. */
  agent_metadata?: AgentSessionMetadata | null;
  /** Transcript-reported cumulative spend and latest context pressure. */
  agent_usage?: AgentSessionUsage | null;
  /** Backend-owned operational state for the live PTY/agent process. */
  activity?: SessionActivity;
  /** Published plan currently attached to this PTY, if any. */
  current_plan?: CurrentPlanView | null;
  /** Count of queued `pending` future prompts for the session's
   * currently correlated transcript session. Drives the sidebar
   * future-prompts badge. 0 when there's no correlated session. */
  future_prompts_pending_count: number;
  /** Timeline-submitted prompts that no transcript turn has claimed yet.
   * Drives the prompt-bar badge that opens the submitted-prompts window. */
  unmatched_prompt_count?: number;
  /** Oldest model change the user has not confirmed in the current
   * transcript session. The timeline opens its confirmation dialog on it. */
  pending_model_switch?: ModelSwitchView | null;
}

/** Which transcript record revealed a model change. */
export type ModelSwitchSource =
  | "codex_thread_settings"
  | "codex_turn_context"
  | "claude_fallback"
  | "claude_message"
  | string;

/** Codex's rate-limit snapshot, as it appears in `token_count` records. */
export interface CodexRateLimitWindow {
  used_percent: number;
  window_minutes: number;
  /** Unix seconds. */
  resets_at: number;
}

export interface CodexRateLimits {
  limit_id?: string | null;
  plan_type?: string | null;
  rate_limit_reached_type?: string | null;
  primary?: CodexRateLimitWindow | null;
  secondary?: CodexRateLimitWindow | null;
  credits?: {
    has_credits?: boolean;
    unlimited?: boolean;
    balance?: string | null;
  } | null;
}

/** Whatever the transcript held near a switch that may explain it. The
 * harnesses record no reason; this is the surrounding evidence. */
export interface ModelSwitchContext {
  /** Codex: the last rate-limit snapshot before the switch. */
  rate_limits?: CodexRateLimits | null;
  /** Claude: the `fallback` content block's models. */
  fallback?: { from: string | null; to: string | null } | null;
  /** Claude: per-request iterations, showing the primary attempt and the
   * fallback retry. */
  iterations?: Array<{ type: string | null; model: string | null }> | null;
  service_tier?: string | null;
  model_provider_id?: string | null;
}

export interface ModelSwitchView {
  id: string;
  agent: string;
  source: ModelSwitchSource;
  from_model: string | null;
  to_model: string;
  from_effort: string | null;
  to_effort: string | null;
  turn_id: string | null;
  /** A turn was underway when the change was observed. */
  turn_in_flight: boolean;
  context: ModelSwitchContext;
  observed_at: string;
  /** When the guard's interrupt reached the harness. */
  interrupted_at: string | null;
  interrupt_error: string | null;
}

export interface ModelSwitchRecord extends ModelSwitchView {
  session_uuid: string;
  enforced: boolean;
  detected_at: string;
  acknowledged_at: string | null;
  adopted: boolean | null;
}

export interface ModelSwitchListResponse {
  switches: ModelSwitchRecord[];
}

export interface SessionMetaRepoView {
  id: string;
  name: string;
}

export interface SessionWorkspaceView {
  id: string;
  repo_name: string;
  kind: "main" | "worktree" | string;
  path: string;
  branch_name: string | null;
  base_ref: string | null;
  base_sha: string | null;
  merge_target: string | null;
}

export type SessionColor =
  | "amber"
  | "emerald"
  | "sky"
  | "rose"
  | "violet"
  | "slate"
  | "teal"
  | "fuchsia";

export const SESSION_COLORS: readonly SessionColor[] = [
  "amber",
  "emerald",
  "sky",
  "rose",
  "violet",
  "slate",
  "teal",
  "fuchsia",
] as const;

export interface UpdateSessionRequest {
  label?: string | null;
  pinned?: boolean;
  color?: SessionColor | null;
}

export interface CreateSessionRequest {
  repo?: string;
  meta_repo_id?: string;
  working_dir?: string;
  workspace_id?: string;
  workspace_mode?: "main" | "isolated";
  cols?: number;
  rows?: number;
  /** Resume session id when the backend supports agent-specific resume. */
  resume_session_uuid?: string;
  /** Agent id for `resume_session_uuid`. */
  resume_agent?: string;
  /** Agent to launch immediately in the new PTY. */
  launch_agent?: AgentLaunchType;
}

export interface RepoView {
  name: string;
  path: string;
  exists?: boolean;
  timeline_revision?: number;
  git?: RepoGitSummary | null;
}

export interface MetaRepoMemberView {
  repo_name: string;
  exists: boolean;
}

export interface MetaRepoView {
  id: string;
  name: string;
  primary_repo_name: string;
  members: MetaRepoMemberView[];
  created_at: string;
  updated_at: string;
}

export interface SaveMetaRepoRequest {
  name: string;
  members: string[];
  primary_repo_name: string;
}

export type NodeConnectionState =
  | "pending"
  | "enrolled"
  | "connected"
  | "disconnected";

export interface NodeView {
  id: string;
  display_name: string;
  protocol_version: number | null;
  boot_id: string | null;
  connection_state: NodeConnectionState;
  connected_at: string | null;
  last_heartbeat_at: string | null;
  node_disconnected_at: string | null;
  heartbeat_timeout_seconds: number;
  pending_key_fingerprint: string | null;
}

export interface AppStateResponse {
  generated_at: string;
  /** Optional during rolling upgrades from pre-node control releases. */
  nodes?: NodeView[];
  sessions: SessionView[];
  repos: RepoView[];
  meta_repos?: MetaRepoView[];
  workspaces?: WorkspaceView[];
  plans?: PlanSummaryView[];
  stats: StatsResponse;
}

export type PlanStatus = "active" | "paused" | "completed" | "canceled";
export type PlanPhaseStatus =
  | "pending"
  | "in_progress"
  | "blocked"
  | "completed"
  | "skipped";

export interface PlanPhaseView {
  id: string;
  plan_id: string;
  position: number;
  title: string;
  description: string;
  status: PlanPhaseStatus;
  status_note: string | null;
  /** Optional t-shirt weight for burndown (s=1, m=2, l=3). */
  size: "s" | "m" | "l" | null;
  started_at: string | null;
  completed_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface PlanAttachmentView {
  pty_session_id: string;
  agent_session_uuid: string | null;
  attached_at: string;
}

export interface PlanGuidance {
  outcome: string;
  principles: string[];
  assumptions: string[];
}

export interface PlanAncestorView extends PlanGuidance {
  id: string;
  title: string;
  status: PlanStatus;
  depth: number;
  revision: number;
}

/** A direct sub-plan, with the parent phases it covers. */
export interface PlanBranchView {
  id: string;
  title: string;
  summary: string;
  status: PlanStatus;
  depth: number;
  total_phases: number;
  completed_phases: number;
  anchor_phase_ids: string[];
}

export interface PlanTreeNodeView {
  id: string;
  title: string;
  status: PlanStatus;
  depth: number;
  parent_plan_id: string | null;
  total_phases: number;
  completed_phases: number;
  blocked_phases: number;
  attached_pty_ids: string[];
}

export interface PlanView extends PlanGuidance {
  id: string;
  repo_name: string;
  title: string;
  summary: string;
  status: PlanStatus;
  revision: number;
  parent_plan_id: string | null;
  root_plan_id: string;
  depth: number;
  created_by_pty_id: string | null;
  created_by_agent_session_uuid: string | null;
  created_at: string;
  updated_at: string;
  closed_at: string | null;
  phases: PlanPhaseView[];
  attachments: PlanAttachmentView[];
  /** Phases in the parent plan this plan covers; empty for a root. */
  anchor_phase_ids: string[];
  /** Root first, immediate parent last; empty for a root. */
  ancestors: PlanAncestorView[];
  /** Direct sub-plans, open ones first. */
  branches: PlanBranchView[];
}

export interface BranchPlanRequest extends Partial<PlanGuidance> {
  title: string;
  summary?: string;
  phases: NewPlanPhaseInput[];
  all_pending?: boolean;
  parent_phase_refs?: string[];
  note?: string;
}

export interface PlanSummaryView {
  id: string;
  repo_name: string;
  title: string;
  summary: string;
  status: PlanStatus;
  revision: number;
  total_phases: number;
  completed_phases: number;
  blocked_phases: number;
  current_phase_id: string | null;
  current_phase_title: string | null;
  current_phase_status: PlanPhaseStatus | null;
  attached_pty_ids: string[];
  parent_plan_id: string | null;
  root_plan_id: string;
  depth: number;
  open_branches: number;
  updated_at: string;
}

export interface PlanEventView {
  id: number;
  plan_id: string;
  phase_id: string | null;
  event_type: string;
  actor_kind: "agent" | "user" | "system";
  pty_session_id: string | null;
  agent_session_uuid: string | null;
  from_status: string | null;
  to_status: string | null;
  note: string | null;
  guidance_before: PlanGuidance | null;
  guidance_after: PlanGuidance | null;
  created_at: string;
}

export interface NewPlanPhaseInput {
  title: string;
  description?: string;
  status?: PlanPhaseStatus;
}

export interface CreatePlanInput extends Partial<PlanGuidance> {
  title: string;
  summary?: string;
  phases: NewPlanPhaseInput[];
  all_pending?: boolean;
  attach_pty_id?: string;
}

export interface UpdatePlanInput extends Partial<PlanGuidance> {
  title?: string;
  summary?: string;
  status?: PlanStatus;
  note?: string;
  skip_remaining?: boolean;
}

export interface UpdatePlanPhaseInput {
  title?: string;
  description?: string;
  status?: PlanPhaseStatus;
  status_note?: string;
  position?: number;
}

export interface WorkspaceView {
  id: string;
  repo_name: string;
  kind: "main" | "worktree" | string;
  path: string;
  branch_name: string | null;
  base_ref: string | null;
  base_sha: string | null;
  merge_target: string | null;
  created_by_session_id: string | null;
  state: "active" | "missing" | "deleted" | string;
  created_at: string;
  updated_at: string;
  git: RepoGitSummary;
}

export interface SecretMetadata {
  id: string;
  description: string;
  scope: string;
  repo: string | null;
  env_keys: string[];
  updated_at: string;
}

export interface SecretEnvelope {
  description: string;
  scope: string;
  repo: string | null;
  env: Record<string, string>;
}

export interface SecretGrantMetadata {
  secret_id: string;
  granted_by_sub: string;
  granted_by_username: string | null;
  expires_at: string;
}

export interface CreateRepoRequest {
  name: string;
  git_url?: string;
}

export interface RenameRepoRequest {
  name: string;
}

export type OperationCategory =
  | "create_content"
  | "inspect"
  | "utility"
  | "research"
  | "delegate"
  | "workflow"
  | "other";

/** One canonical content block. Agent-agnostic: same shape whether
 * the source is Claude, Codex, or any future parser. `tool_name`
 * preserves the raw emitted name; `tool_name_canonical` is what the
 * renderers switch on, while `operation_category` is the coarser
 * app-facing grouping projected by the backend from ref-data rules.
 * The API intentionally omits any raw per-block JSON to force
 * consumers onto the canonical form. */
export interface TimelineBlock {
  ord: number;
  kind: "text" | "thinking" | "tool_use" | "tool_result" | "unknown";
  text?: string;
  tool_id?: string;
  tool_name?: string;
  tool_name_canonical?: string;
  operation_type?: string;
  operation_category?: OperationCategory;
  tool_input?: unknown;
  tool_output?: unknown;
  is_error?: boolean;
}

export interface TimelineEvent {
  byte_offset: number;
  timestamp: string;
  kind: string;
  /** Ingesting agent id — "claude-code", "codex", etc. */
  agent: string;
  /** Normalised speaker: user / assistant / system / summary / other. */
  speaker: string | null;
  /** Coarse content-kind discriminator for quick filtering without
   * walking `blocks`. */
  content_kind: string | null;
  /** Stable event id emitted by the source transcript, when present. */
  event_uuid: string | null;
  /** Parent event id for sidechain/subagent lineage, when present. */
  parent_event_uuid: string | null;
  /** Related tool_use id carried by some result/report rows. */
  related_tool_use_id: string | null;
  /** True when this event belongs to a Task-subagent conversation. */
  is_sidechain: boolean;
  /** True for internal/bookkeeping system events. */
  is_meta: boolean;
  /** Optional subtype for system/bookkeeping rows. */
  subtype: string | null;
  /** Canonical content blocks, emitted by the ingester's parser. */
  blocks: TimelineBlock[];
}

export interface HistoryResponse {
  session_uuid: string | null;
  session_agent: string | null;
  events: TimelineEvent[];
  next_after: number | null;
}

export interface HistoryQuery {
  after?: number;
  limit?: number;
  kind?: string;
  session?: string;
}

export type SpeakerFacet = "user" | "assistant" | "tool_result";

export interface TimelineQuery {
  session?: string;
  hidden_speakers?: SpeakerFacet[];
  hidden_operation_categories?: OperationCategory[];
  errors_only?: boolean;
  show_bookkeeping?: boolean;
  show_sidechain?: boolean;
  file_path?: string;
}

export interface MonitorTimelineRequest extends TimelineQuery {
  session_ids: string[];
}

export interface TimelineOperationBadge {
  name: string;
  operation_type?: string | null;
  count: number;
  /** Operations in this badge whose result has not arrived yet. */
  pending_count?: number;
}

export interface TimelineToolResult {
  content?: string | null;
  payload?: unknown | null;
  is_error: boolean;
}

export interface TimelineFileTouch {
  repo: string;
  path: string;
  touch_kind: string;
  is_write: boolean;
}

/** A spawning call's transcript, by reference: a whole child session, or
 * the listed sidechain turns of one. Fetch the turns with
 * `getSessionTurns`. */
export interface TimelineSubagent {
  title: string;
  event_count: number;
  turn_count: number;
  session_uuid?: string | null;
  turn_ids?: number[];
}

export interface TimelineToolPair {
  /** Stream metadata omits bodies until a tool is opened. */
  body_loaded?: boolean;
  body_version?: number;
  id: string;
  name: string;
  raw_name?: string | null;
  operation_type?: string | null;
  category?: OperationCategory | null;
  input?: unknown;
  result?: TimelineToolResult | null;
  is_error: boolean;
  is_pending: boolean;
  file_touches: TimelineFileTouch[];
  subagent?: TimelineSubagent | null;
}

export type TimelineAssistantItem =
  | { kind: "text"; text: string }
  | { kind: "tool"; pair_id: string };

export type TimelineChunk =
  | { kind: "assistant"; items: TimelineAssistantItem[]; thinking: string[] }
  | { kind: "summary"; subtype: string | null; text: string }
  | { kind: "system"; subtype: string | null; text: string; is_meta: boolean }
  | {
      kind: "generic";
      label: string;
      details: {
        event_uuid: string | null;
        parent_event_uuid: string | null;
        related_tool_use_id: string | null;
        subtype: string | null;
        speaker: string | null;
        content_kind: string | null;
        blocks: TimelineBlock[];
      };
    };

/** One visible event of a turn, keyed by its transcript byte offset. Items
 * are written once and never change; consecutive assistant items render as
 * one block (see `groupItems`). */
export type TimelineItem = TimelineChunk & { offset: number };

export interface TimelineTurn {
  generation?: string;
  id: number;
  turn_key?: string | null;
  preview: string;
  user_prompt_text?: string | null;
  start_timestamp: string;
  end_timestamp: string;
  duration_ms: number;
  event_count: number;
  operation_count: number;
  tool_pairs: TimelineToolPair[];
  thinking_count: number;
  has_errors: boolean;
  is_sidechain?: boolean;
  input_tokens?: number;
  output_tokens?: number;
  markdown: string;
  items: TimelineItem[];
  pty_session_id?: string | null;
  session_uuid?: string | null;
  session_agent?: string | null;
  session_label?: string | null;
  session_state?: SessionState | null;
  /** Set by the client from the detail response when the session was
   * purged to its turn digest: `items` is empty and `markdown` is the
   * whole record until the session is restored. */
  archived_at?: string | null;
}

export interface TimelineTurnSummary {
  id: number;
  turn_key?: string | null;
  preview: string;
  start_timestamp: string;
  end_timestamp: string;
  duration_ms: number;
  event_count: number;
  operation_count: number;
  operation_badges: TimelineOperationBadge[];
  thinking_count: number;
  has_errors: boolean;
  is_sidechain?: boolean;
  input_tokens?: number;
  output_tokens?: number;
  pty_session_id?: string | null;
  session_uuid?: string | null;
  session_agent?: string | null;
  session_label?: string | null;
  session_state?: SessionState | null;
}

export interface TimelineSummaryResponse {
  session_uuid: string | null;
  session_agent: string | null;
  total_event_count: number;
  turns: TimelineTurnSummary[];
  /** The session was purged to its turn digest on this date. */
  archived_at?: string | null;
}

export interface TimelineTurnDetailResponse {
  session_uuid: string;
  session_agent: string | null;
  turn: TimelineTurn;
  archived_at?: string | null;
  /** Transcript offset the read reflects; pass it back as `since`. */
  through: number;
  /** Set on a `since` read: `turn` holds only the items and tool pairs
   * changed by events after that offset, and no markdown. */
  since?: number | null;
}

export interface SessionTurnsResponse {
  session_uuid: string;
  session_agent: string | null;
  through: number;
  turns: TimelineTurn[];
}

export interface MonitorSessionTurn {
  pty_session_id: string;
  repo: string;
  label: string | null;
  pty_state: SessionState;
  current_session_uuid: string | null;
  current_session_agent: string | null;
  total_event_count: number;
  turn: TimelineTurn | null;
}

export interface MonitorTimelineResponse {
  generated_at: string;
  sessions: MonitorSessionTurn[];
}

export interface GitCommit {
  sha: string;
  subject: string;
  committed_at: string;
}

export interface GitStatus {
  branch: string | null;
  uncommitted_count: number;
  untracked_count: number;
  last_commit: GitCommit | null;
  recent_commits: GitCommit[];
  /** Repo-relative path → 2-char status code. */
  dirty_by_path: Record<string, string>;
  /** Repo-relative path → current working-copy churn. */
  diff_stats_by_path: Record<string, DiffStat>;
}

export interface RepoGitSummary {
  revision: number;
  branch: string | null;
  uncommitted_count: number;
  untracked_count: number;
  last_commit: GitCommit | null;
  recent_commits: GitCommit[];
  refreshing: boolean;
  status_error: string | null;
}

export interface RepoDirtyPathsResponse {
  repo: string;
  git_revision: number;
  dirty_by_path: Record<string, string>;
  diff_stats_by_path: Record<string, DiffStat>;
}

export interface WorkspaceDirtyPathsResponse {
  workspace_id: string;
  git_revision: number;
  dirty_by_path: Record<string, string>;
  diff_stats_by_path: Record<string, DiffStat>;
}

export interface DiffStat {
  additions: number;
  deletions: number;
}

export interface DirEntryView {
  name: string;
  kind: "file" | "dir";
  size: number;
  mtime: string | null;
  dirty: string | null;
  diff?: DiffStat | null;
}

export interface DirListing {
  path: string;
  entries: DirEntryView[];
}

export interface FileResponse {
  path: string;
  size: number;
  mime: string;
  binary: boolean;
  truncated: boolean;
  content: string | null;
}

export interface DiffResponse {
  diff: string;
}

export interface FileTraceTouch {
  pty_session_id: string | null;
  session_uuid: string;
  session_agent: string | null;
  session_label: string | null;
  session_state: SessionState | null;
  turn_id: number;
  turn_preview: string;
  turn_timestamp: string;
  operation_type: string | null;
  operation_category: OperationCategory | null;
  /** Tool-call id this touch belongs to. Null when the touch has no
   * specific tool (e.g. plain user-prompt turns). Lets the client jump
   * to the exact tool row inside the turn. */
  pair_id: string | null;
  touch_kind: string;
  is_write: boolean;
}

export interface FileTraceResponse {
  path: string;
  dirty: string | null;
  current_diff: DiffStat | null;
  touches: FileTraceTouch[];
}

export interface StatsResponse {
  uptime_seconds: number;
  /** Whole-machine pressure on the development node, from its latest
   * heartbeat. Null until a node reports one, and again once it disconnects. */
  node: {
    memory_used_bytes: number;
    memory_total_bytes: number;
    cpu_percent: number;
  } | null;
  /** The node's PTY path: whether the devenv that hosts new shells is dialed
   * in. Null while no node has reported one (including a node release that
   * predates the field), so absence must not render as an outage. */
  devenv: {
    current_ident: string;
    current_connected: boolean;
    connected_idents: string[];
  } | null;
  pty: {
    live_sessions: number;
    live_agent_sessions: number;
  };
  db: {
    database_size_bytes: number;
  };
  ingest: {
    last_tick_started_at_unix: number | null;
    last_progress_at_unix: number | null;
    stalled_seconds: number | null;
  };
  inventory: {
    event_rows: number;
    agent_sessions: number;
    pty_sessions: number;
    tracked_files: number;
    events_inserted_since_boot: number;
    parse_errors_since_boot: number;
  };
}

/** One global library entry. References store assistant output for
 * later rereading; prompts store reusable user instructions. */
export interface LibraryEntry {
  slug: string;
  name: string;
  created_at: string | null;
  updated_at: string | null;
  body: string;
}

export type LibraryKind = "references" | "prompts";

export interface SaveLibraryInput {
  name: string;
  body: string;
}

export type FuturePromptState = "pending" | "sent";

export interface FuturePromptEntry {
  id: string;
  state: FuturePromptState;
  created_at: string | null;
  updated_at: string | null;
  text: string;
}

export interface FuturePromptListResponse {
  session_uuid: string | null;
  session_agent: string | null;
  prompts: FuturePromptEntry[];
}

export interface CreateFuturePromptInput {
  text: string;
}

/** Why the timeline input is closed for a PTY. */
export type PromptGate = "starting" | "needs_input" | "blocked";

export type SubmittedPromptState = "matched" | "unmatched" | "failed" | "dismissed";

/** A prompt sent from the timeline input, recorded before it reached the
 * PTY. `matched` once the ingester projected a turn with the same text. */
export interface SubmittedPrompt {
  id: string;
  agent: string | null;
  text: string;
  forced: boolean;
  submitted_at: string;
  state: SubmittedPromptState;
  delivery_error: string | null;
  /** `<session_uuid>:<turn_id>` of the turn this prompt became. */
  matched_turn_key: string | null;
  matched_at: string | null;
  dismissed_at: string | null;
}

export interface SubmittedPromptListResponse {
  gate: PromptGate | null;
  prompts: SubmittedPrompt[];
}

export interface UpdateFuturePromptInput {
  text?: string;
  state?: FuturePromptState;
}

// ─── Portfolio metrics (`/api/metrics`) ─────────────────────────────

export interface UsageWindowView {
  /** Standard input plus cache writes; cache reads are excluded. */
  input_tokens: number;
  /** Cache-write subset of input_tokens. */
  cache_write_input_tokens: number;
  cached_input_tokens: number;
  output_tokens: number;
  estimated_cost_usd: number;
  unpriced_tokens: number;
}

export interface MetricsUsageDay {
  day: string;
  input_tokens: number;
  cache_write_input_tokens: number;
  cached_input_tokens: number;
  output_tokens: number;
  estimated_cost_usd: number;
  unpriced_tokens: number;
}

export interface MetricsRepoUsage {
  repo: string;
  all_time: UsageWindowView;
  today: UsageWindowView;
  last_7d: UsageWindowView;
}

export interface MetricsUsage {
  all_time: UsageWindowView;
  today: UsageWindowView;
  last_7d: UsageWindowView;
  per_repo: MetricsRepoUsage[];
  by_model: MetricsModelUsage[];
  model_window_days: number;
  daily: MetricsUsageDay[];
  pricing: MetricsUsagePricing;
}

export interface MetricsModelPrice {
  input_usd_per_million: number;
  cached_input_usd_per_million: number;
  cache_write_usd_per_million: number;
  cache_write_1h_usd_per_million: number;
  output_usd_per_million: number;
}

export interface MetricsModelUsage {
  model: string;
  agent: string;
  usage: UsageWindowView;
  price: MetricsModelPrice | null;
}

export interface MetricsUsagePricing {
  basis: string;
  as_of: string;
  openai_source_url: string;
  anthropic_source_url: string;
  note: string;
}

export interface MetricsGitDay {
  day: string;
  commits: number;
  insertions: number;
  deletions: number;
}

export interface RepoGitActivityView {
  repo: string;
  commits_24h: number;
  commits_7d: number;
  insertions_24h: number;
  deletions_24h: number;
  insertions_7d: number;
  deletions_7d: number;
  agent_commits_7d: number;
  human_commits_7d: number;
  last_commit_at: string | null;
  daily: MetricsGitDay[];
}

export interface ChurnHotspotView {
  repo: string;
  path: string;
  write_turns: number;
  sessions: number;
  last_write_at: string;
}

export interface FlowCfdDay {
  day: string;
  pending: number;
  in_progress: number;
  blocked: number;
  completed: number;
  skipped: number;
}

export interface BurndownDayView {
  day: string;
  remaining_weight: number;
  total_weight: number;
}

export interface PlanBurndownView {
  plan_id: string;
  repo: string;
  title: string;
  total_weight: number;
  days: BurndownDayView[];
}

export interface ThroughputWeekView {
  week_start: string;
  completed_weight: number;
}

export interface FlowMetricsView {
  wip: number;
  blocked: number;
  throughput_weeks: ThroughputWeekView[];
  cycle_time_hours_p50: number | null;
  cfd: FlowCfdDay[];
  burndowns: PlanBurndownView[];
}

export interface MetricsResponse {
  generated_at: string;
  usage: MetricsUsage;
  git: RepoGitActivityView[];
  churn: ChurnHotspotView[];
  flow: FlowMetricsView;
}

export interface JobView {
  id: number;
  name: string;
  label: string;
  status: "running" | "completed" | "failed" | "interrupted";
  progress_current: number;
  progress_total: number | null;
  unit: string;
  detail: string | null;
  error: string | null;
  started_at: string;
  updated_at: string;
  finished_at: string | null;
  stalled: boolean;
}

export interface JobsResponse {
  active: JobView[];
  recent: JobView[];
}
