// Mirrors the Rust SessionView in backend/src/routes.rs. Keep in sync
// manually — a small enough surface that codegen would be overkill.

export type SessionState = "live" | "dead" | "deleted" | "orphaned";
export type AgentLaunchType = "claude" | "codex";
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
  repo: string;
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

export interface AppStateResponse {
  generated_at: string;
  sessions: SessionView[];
  repos: RepoView[];
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

export interface PlanView {
  id: string;
  repo_name: string;
  title: string;
  summary: string;
  status: PlanStatus;
  revision: number;
  created_by_pty_id: string | null;
  created_by_agent_session_uuid: string | null;
  created_at: string;
  updated_at: string;
  closed_at: string | null;
  phases: PlanPhaseView[];
  attachments: PlanAttachmentView[];
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
  created_at: string;
}

export interface NewPlanPhaseInput {
  title: string;
  description?: string;
  status?: PlanPhaseStatus;
}

export interface CreatePlanInput {
  title: string;
  summary?: string;
  phases: NewPlanPhaseInput[];
  all_pending?: boolean;
  attach_pty_id?: string;
}

export interface UpdatePlanInput {
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

export interface TimelineSubagent {
  title: string;
  event_count: number;
  turns: TimelineTurn[];
}

export interface TimelineToolPair {
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
  | { kind: "tool"; pair_id: string }
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

export interface TimelineTurn {
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
  markdown: string;
  chunks: TimelineChunk[];
  pty_session_id?: string | null;
  session_uuid?: string | null;
  session_agent?: string | null;
  session_label?: string | null;
  session_state?: SessionState | null;
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
}

export interface TimelineTurnDetailResponse {
  session_uuid: string;
  session_agent: string | null;
  turn: TimelineTurn;
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
  process: {
    memory_rss_bytes: number;
    cpu_percent: number;
    memory_limit_bytes: number | null;
  };
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

export interface UpdateFuturePromptInput {
  text?: string;
  state?: FuturePromptState;
}

// ─── Portfolio metrics (`/api/metrics`) ─────────────────────────────

export interface UsageWindowView {
  fresh_tokens: number;
  cached_tokens: number;
  total_tokens: number;
}

export interface MetricsUsageDay {
  day: string;
  fresh_tokens: number;
  cached_tokens: number;
  total_tokens: number;
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
  daily: MetricsUsageDay[];
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
