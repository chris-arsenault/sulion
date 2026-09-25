// Renders backend-projected timeline summaries and selected turn detail.
// The unified app-state poll carries timeline revision markers; this
// pane fetches summaries only when its active resource revision changes.
// On mobile the detail replaces the list inline so prompt controls remain
// available; intermediate-width viewports use an overlay instead of a side
// pane.
//
// The inspector's TurnDetail is reused by the SubagentModal so drill-in
// into sidechain logs renders the same way.

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type MouseEvent as ReactMouseEvent,
  type MutableRefObject,
} from "react";
import { Virtuoso, type VirtuosoHandle } from "react-virtuoso";

import {
  getRepoTimeline,
  getRepoTimelineTurn,
  getTimeline,
  getTimelineTurn,
  interruptSessionAgent,
  sendSessionPrompt,
  startSessionAgent,
} from "../api/client";
import { PasteUploadDialog, type PendingAttachment } from "./common/PasteUploadDialog";
import {
  createClipboardImageUpload,
  imageFromClipboard,
  PASTE_AS_FILE_BYTES,
  PASTE_AS_FILE_LINES,
} from "./terminal/clipboard";
import type {
  AgentLaunchType,
  SessionView,
  TimelineQuery,
  TimelineSummaryResponse,
} from "../api/types";
import { useMediaQuery } from "../hooks/useMediaQuery";
import { MOBILE_LAYOUT_QUERY } from "../state/displayPolicy";
import { useTimelineFontScale, useTurnNavMode } from "../state/paneTextScale";
import { appCommands, useAppCommand } from "../state/AppCommands";
import { usePromptInjectionTarget } from "../hooks/usePromptInjectionTarget";
import { useSessions } from "../state/SessionStore";
import { useTabs } from "../state/TabStore";
import { useDisplay } from "../state/DisplayStore";
import { useTimelineFilters } from "./timeline/filters";
import { gateText, promptGateFor } from "./timeline/promptGate";
import { type Turn, type TurnSummary } from "./timeline/grouping";
import type { FileLinkTarget } from "./timeline/markdownLinks";
import { ModelSwitchGuard } from "./timeline/ModelSwitchModal";
import { SessionInspectorPane } from "./timeline/SessionInspectorPane";
import { SubagentModal } from "./timeline/SubagentModal";
import { applyTurnDetail, type TurnDetailEntry } from "./timeline/turnDetailCache";
import { useSubagentStack } from "./timeline/useSubagentStack";
import { TimelineControlsFlyout } from "./timeline/TimelineControlsFlyout";
import { TurnGridFlyout } from "./timeline/TurnGridFlyout";
import { TurnRow } from "./timeline/TurnRow";
import { Icon } from "../icons";
import { Tooltip } from "./ui";
import "./TimelinePane.css";

const INSPECTOR_WIDTH_KEY = "sulion.timeline.inspector.width.v1";
const DEFAULT_INSPECTOR_FRACTION = 0.55;
const MIN_INSPECTOR_FRACTION = 0.28;
const MAX_INSPECTOR_FRACTION = 0.78;

interface CachedTurnDetail extends TurnDetailEntry {
  fingerprint: string;
  /** App-state timeline revision the entry was read at. A newer one asks
   * for the records changed since: a late result can change an older turn
   * without moving its summary. */
  resourceRevision: number;
}

export function TimelinePane({
  tabId,
  sessionId,
  repo,
  active = true,
  focusTurnId,
  focusPairId,
  focusKey,
}: {
  tabId?: string;
  sessionId?: string;
  repo?: string;
  active?: boolean;
  focusTurnId?: number;
  focusPairId?: string;
  focusKey?: string;
}) {
  const [timeline, setTimeline] = useState<TimelineSummaryResponse | null>(null);
  const [detailCache, setDetailCache] = useState<Map<string, CachedTurnDetail>>(
    () => new Map(),
  );
  const [currentSessionUuid, setCurrentSessionUuid] = useState<string | null>(null);
  const [currentSessionAgent, setCurrentSessionAgent] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);
  const virtuoso = useRef<VirtuosoHandle | null>(null);
  const [selectedTurnKey, setSelectedTurnKey] = useState<string | null>(null);
  const appliedFocusKeyRef = useRef<string | null>(null);
  const loadedSummaryKeyRef = useRef<string | null>(null);

  const { filters, setFollowLatest } = useTimelineFilters();
  const narrow = useMediaQuery("(max-width: 999px)");
  const isMobile = useMediaQuery(MOBILE_LAYOUT_QUERY);
  const [turnNavMode] = useTurnNavMode();
  const [timelineFontScale] = useTimelineFontScale();
  const resourceRevision = useSessions((store) => {
    if (sessionId) {
      return store.sessions.find((session) => session.id === sessionId)?.timeline_revision ?? 0;
    }
    if (repo) {
      return store.repos.find((candidate) => candidate.name === repo)?.timeline_revision ?? 0;
    }
    return 0;
  });
  const session = useSessions((store) =>
    sessionId ? store.sessions.find((candidate) => candidate.id === sessionId) : undefined,
  );
  const refreshSessions = useSessions((store) => store.refresh);

  // Relative links in turn markdown open files from the session's repo
  // (and its isolated workspace when it has one); a repo timeline uses
  // the repo itself.
  const fileTargetRepo = session?.repo ?? repo ?? null;
  const fileTargetWorkspaceId = session?.workspace?.id;
  const fileTarget = useMemo<FileLinkTarget | null>(
    () =>
      fileTargetRepo
        ? { repo: fileTargetRepo, workspaceId: fileTargetWorkspaceId }
        : null,
    [fileTargetRepo, fileTargetWorkspaceId],
  );

  const [inspectorFraction, setInspectorFraction] = useState<number>(() => {
    if (typeof window === "undefined") return DEFAULT_INSPECTOR_FRACTION;
    const raw = window.localStorage.getItem(INSPECTOR_WIDTH_KEY);
    const n = raw ? Number(raw) : NaN;
    if (Number.isFinite(n) && n >= MIN_INSPECTOR_FRACTION && n <= MAX_INSPECTOR_FRACTION) {
      return n;
    }
    return DEFAULT_INSPECTOR_FRACTION;
  });
  useEffect(() => {
    window.localStorage.setItem(INSPECTOR_WIDTH_KEY, String(inspectorFraction));
  }, [inspectorFraction]);

  const query = useMemo<TimelineQuery>(
    () => ({
      hidden_speakers: Array.from(filters.hiddenSpeakers),
      hidden_operation_categories: Array.from(filters.hiddenOperationCategories),
      errors_only: filters.errorsOnly,
      show_bookkeeping: filters.showBookkeeping,
      show_sidechain: filters.showSidechain,
      file_path: filters.filePath || undefined,
    }),
    [filters],
  );
  const queryKey = useMemo(
    () =>
      JSON.stringify({
        hidden_speakers: [...filters.hiddenSpeakers].sort(),
        hidden_operation_categories: [...filters.hiddenOperationCategories].sort(),
        errors_only: filters.errorsOnly,
        show_bookkeeping: filters.showBookkeeping,
        show_sidechain: filters.showSidechain,
        file_path: filters.filePath,
      }),
    [filters],
  );
  const subagents = useSubagentStack(query, resourceRevision, active);
  const { close: closeSubagent } = subagents;

  useEffect(() => {
    setTimeline(null);
    setCurrentSessionUuid(null);
    setCurrentSessionAgent(null);
    setLoadError(null);
    setDetailError(null);
    setDetailCache(new Map());
    closeSubagent();
    setSelectedTurnKey(null);
    appliedFocusKeyRef.current = null;
    loadedSummaryKeyRef.current = null;
  }, [sessionId, repo, closeSubagent]);

  useEffect(() => {
    setDetailCache(new Map());
    setDetailError(null);
  }, [queryKey]);

  useEffect(() => {
    if (!active || (!sessionId && !repo)) return;
    const summaryKey = `${sessionId ?? ""}:${repo ?? ""}:${resourceRevision}:${queryKey}`;
    if (loadedSummaryKeyRef.current === summaryKey) return;
    let cancelled = false;
    const load = async () => {
      if (cancelled) return;
      try {
        const resp = sessionId
          ? await getTimeline(sessionId, query)
          : await getRepoTimeline(repo!, query);
        if (cancelled) return;
        loadedSummaryKeyRef.current = summaryKey;
        setCurrentSessionUuid(resp.session_uuid);
        setCurrentSessionAgent(resp.session_agent);
        setTimeline(resp);
        setLoadError(null);
      } catch (err) {
        if (!cancelled) {
          setLoadError(err instanceof Error ? err.message : "timeline fetch failed");
        }
      }
    };

    void load();
    return () => {
      cancelled = true;
    };
  }, [active, sessionId, repo, resourceRevision, query, queryKey]);

  const turns = useMemo<TurnSummary[]>(
    () => timeline?.turns ?? [],
    [timeline],
  );

  // Apply a focus request exactly once per focusKey. `turns` stays in
  // deps so we retry across revision updates if the target turn hasn't been
  // ingested yet, but the ref guard prevents later summary refreshes
  // from stomping on a selection the user has since moved.
  useEffect(() => {
    if (focusTurnId == null || !focusKey) return;
    if (appliedFocusKeyRef.current === focusKey) return;
    const exists = turns.findIndex((turn) => turn.id === focusTurnId);
    if (exists === -1) return;
    appliedFocusKeyRef.current = focusKey;
    setSelectedTurnKey(turnIdentity(turns[exists]!));
    virtuoso.current?.scrollToIndex({
      index: exists,
      align: "center",
      behavior: "auto",
    });
  }, [focusKey, focusTurnId, turns]);

  const selectedSummary = useMemo<TurnSummary | null>(
    () =>
      selectedTurnKey == null
        ? null
        : turns.find((t) => turnIdentity(t) === selectedTurnKey) ?? null,
    [selectedTurnKey, turns],
  );
  const selectedTurn = useMemo<Turn | null>(
    () =>
      selectedTurnKey == null || selectedSummary == null
        ? null
        : detailCache.get(selectedTurnKey)?.turn ?? null,
    [detailCache, selectedSummary, selectedTurnKey],
  );
  const selectedFingerprint = selectedSummary
    ? turnSummaryFingerprint(selectedSummary)
    : null;
  const detailPending =
    selectedSummary != null && selectedTurn == null && !detailError;

  // A turn listed from a child session (the sidechain view) is read from
  // that session, not the pane's.
  const readSelectedTurn = useCallback(
    (summary: TurnSummary, since?: number) => {
      if (sessionId) {
        const childSession =
          summary.session_uuid && summary.session_uuid !== currentSessionUuid
            ? summary.session_uuid
            : undefined;
        const turnQuery = childSession ? { ...query, session: childSession } : query;
        return getTimelineTurn(sessionId, summary.id, turnQuery, since);
      }
      return getRepoTimelineTurn(repo!, summary.session_uuid!, summary.id, query, since);
    },
    [currentSessionUuid, query, repo, sessionId],
  );

  useEffect(() => {
    if (!active || !selectedSummary || !selectedTurnKey) return;
    if (selectedFingerprint == null) return;
    const cached = detailCache.get(selectedTurnKey);
    const cacheFresh =
      cached?.fingerprint === selectedFingerprint &&
      cached.resourceRevision === resourceRevision;
    if (cacheFresh) return;
    if (!sessionId && (!repo || !selectedSummary.session_uuid)) return;

    let cancelled = false;
    const fetchDetail = async () => {
      try {
        const resp = await readSelectedTurn(selectedSummary, cached?.through);
        if (cancelled) return;
        setDetailCache((prev) => {
          const entry = prev.get(selectedTurnKey);
          const next = new Map(prev);
          const applied = applyTurnDetail(entry, resp);
          if (applied) {
            next.set(selectedTurnKey, {
              ...applied,
              fingerprint: selectedFingerprint,
              resourceRevision,
            });
          } else {
            // Asked against a base that has since moved: read it whole.
            next.delete(selectedTurnKey);
          }
          return next;
        });
        setDetailError(null);
      } catch (err) {
        if (!cancelled) {
          setDetailError(err instanceof Error ? err.message : "turn fetch failed");
        }
      }
    };

    void fetchDetail();
    return () => {
      cancelled = true;
    };
  }, [
    detailCache,
    active,
    readSelectedTurn,
    repo,
    resourceRevision,
    selectedFingerprint,
    selectedSummary,
    selectedTurnKey,
    sessionId,
  ]);

  // A merged turn keeps the digest of its last whole read; copying reads
  // the current one.
  const loadSelectedMarkdown = useCallback(async () => {
    if (!selectedSummary) return "";
    const resp = await readSelectedTurn(selectedSummary);
    return resp.turn.markdown;
  }, [readSelectedTurn, selectedSummary]);

  const handleSubagent = subagents.open;
  const backSubagent = subagents.back;

  // A manual click in the turn list is the user overriding whatever
  // focus the tab was opened with. Strip the focus fields from the
  // tab so later polls (or tab revisits) don't re-apply them — and
  // so the persistent focus outline on a tool row goes away. Also
  // drops follow-latest mode, since the user picking a specific turn
  // contradicts "keep snapping to the newest one".
  const clearTimelineFocus = useTabs((store) => store.clearTimelineFocus);
  const handleTurnSelect = useCallback(
    (key: string) => {
      setSelectedTurnKey(key);
      closeSubagent();
      if (tabId) clearTimelineFocus(tabId);
      if (filters.followLatest) setFollowLatest(false);
    },
    [tabId, clearTimelineFocus, closeSubagent, filters.followLatest, setFollowLatest],
  );

  // Follow-latest: while the filter is on, keep the selection pinned
  // to the most recently arrived turn across summary refreshes. Turn identity is
  // stable, so we only restart the selection when the last-turn key
  // actually changes — avoids fighting unrelated re-renders.
  useEffect(() => {
    if (!filters.followLatest) return;
    const last = turns[turns.length - 1];
    if (!last) return;
    const lastKey = turnIdentity(last);
    setSelectedTurnKey((prev) => (prev === lastKey ? prev : lastKey));
    virtuoso.current?.scrollToIndex({
      index: turns.length - 1,
      align: "end",
      behavior: "auto",
    });
  }, [filters.followLatest, turns]);

  const onDividerMouseDown = useCallback(
    (e: ReactMouseEvent<HTMLDivElement>) => {
      e.preventDefault();
      const container = (e.target as HTMLElement).parentElement;
      if (!container) return;
      const rect = container.getBoundingClientRect();
      const onMove = (ev: MouseEvent) => {
        const fraction = (ev.clientX - rect.left) / rect.width;
        const listFraction = Math.max(
          1 - MAX_INSPECTOR_FRACTION,
          Math.min(1 - MIN_INSPECTOR_FRACTION, fraction),
        );
        setInspectorFraction(1 - listFraction);
      };
      const onUp = () => {
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", onUp);
      };
      window.addEventListener("mousemove", onMove);
      window.addEventListener("mouseup", onUp);
    },
    [],
  );

  const onDividerKeyDown = useCallback((e: React.KeyboardEvent) => {
    const step = e.shiftKey ? 0.1 : 0.03;
    if (e.key === "ArrowLeft") {
      e.preventDefault();
      setInspectorFraction((v) => Math.min(MAX_INSPECTOR_FRACTION, v + step));
    } else if (e.key === "ArrowRight") {
      e.preventDefault();
      setInspectorFraction((v) => Math.max(MIN_INSPECTOR_FRACTION, v - step));
    }
  }, []);

  const listFraction = 1 - inspectorFraction;
  const empty = turns.length === 0;

  const clearSelectedTurn = useCallback(() => setSelectedTurnKey(null), []);
  const paneStyle = useMemo(
    (): CSSProperties =>
      ({
        "--timeline-t-meta": `calc(var(--t-meta) * ${timelineFontScale})`,
        "--timeline-t-ui": `calc(var(--t-ui) * ${timelineFontScale})`,
        "--timeline-t-body": `calc(var(--t-body) * ${timelineFontScale})`,
      }) as CSSProperties,
    [timelineFontScale],
  );
  const splitStyle = useMemo(
    (): CSSProperties => ({
      gridTemplateColumns: `${listFraction}fr 6px ${inspectorFraction}fr`,
    }),
    [listFraction, inspectorFraction],
  );

  return (
    <div
      className="timeline-pane"
      data-testid="timeline-pane"
      // eslint-disable-next-line local/no-inline-styles -- pane-scoped text variables are user preferences, not theme classes
      style={paneStyle}
    >
      <div className="timeline-pane__header">
        <span className="timeline-pane__title">Timeline</span>
        {repo ? (
          <span className="timeline-pane__scope">repo {repo}</span>
        ) : currentSessionUuid ? (
          <Tooltip label={`${currentSessionAgent ?? "session"} ${currentSessionUuid}`}>
            <span className="timeline-pane__session">
              {(currentSessionAgent ?? "session")} {currentSessionUuid.slice(0, 8)}
            </span>
          </Tooltip>
        ) : null}
        <span className="timeline-pane__count tabular">
          {turns.length} turn{turns.length === 1 ? "" : "s"} · {timeline?.total_event_count ?? 0} events
        </span>
        {(loadError || detailError) && (
          <Tooltip label={loadError ?? detailError ?? ""}>
            <span className="timeline-pane__error">error</span>
          </Tooltip>
        )}
      </div>
      {sessionId && session && (
        <>
          <NeedsInputBanner session={session} />
          <ModelSwitchGuard
            sessionId={sessionId}
            session={session}
            active={active}
            onRefresh={refreshSessions}
          />
        </>
      )}
      {empty ? (
        <div className="timeline-pane__empty">
          {(timeline?.total_event_count ?? 0) === 0
            ? repo
              ? "No timeline data for this repo yet."
              : currentSessionUuid
              ? "Waiting for events…"
              : "No transcript session correlated yet."
            : "No turns match current filters."}
        </div>
      ) : narrow ? (
        isMobile && selectedSummary ? (
          <div className="timeline-pane__mobile-detail">
            <div className="timeline-pane__mobile-detail-header">
              <button
                type="button"
                className="timeline-pane__mobile-back"
                onClick={clearSelectedTurn}
              >
                <Icon name="arrow-left" size={14} />
                <span>Back to timeline</span>
              </button>
            </div>
            <SessionInspectorPane
              turn={selectedTurn}
              loading={detailPending}
              showThinking={filters.showThinking}
              hideUserPrompt={filters.hiddenSpeakers.has("user")}
              onOpenSubagent={handleSubagent}
              loadMarkdown={loadSelectedMarkdown}
              asOverlay={false}
              focusPairId={focusPairId ?? null}
              focusKey={focusKey ?? null}
              fileTarget={fileTarget}
            />
          </div>
        ) : (
          <>
            {turnNavMode === "list" && (
              <div className="timeline-pane__list-narrow">
                <TurnList
                  turns={turns}
                  selectedTurnKey={selectedTurnKey}
                  showThinking={filters.showThinking}
                  onSelect={handleTurnSelect}
                  virtuosoRef={virtuoso}
                />
              </div>
            )}
            {!isMobile && (
              <SessionInspectorPane
                turn={selectedTurn}
                loading={detailPending}
                showThinking={filters.showThinking}
                hideUserPrompt={filters.hiddenSpeakers.has("user")}
                onOpenSubagent={handleSubagent}
                loadMarkdown={loadSelectedMarkdown}
                asOverlay
                onClose={clearSelectedTurn}
                focusPairId={focusPairId ?? null}
                focusKey={focusKey ?? null}
                fileTarget={fileTarget}
              />
            )}
          </>
        )
      ) : turnNavMode !== "list" ? (
        <div className="timeline-pane__solo">
          <SessionInspectorPane
            turn={selectedTurn}
            loading={detailPending}
            showThinking={filters.showThinking}
            hideUserPrompt={filters.hiddenSpeakers.has("user")}
            onOpenSubagent={handleSubagent}
            loadMarkdown={loadSelectedMarkdown}
            asOverlay={false}
            focusPairId={focusPairId ?? null}
            focusKey={focusKey ?? null}
            fileTarget={fileTarget}
          />
        </div>
      ) : (
        <div
          className="timeline-pane__split"
          // eslint-disable-next-line local/no-inline-styles -- resizable split fractions are per-user-drag; can't be CSS classes
          style={splitStyle}
        >
          <div className="timeline-pane__list">
            <TurnList
              turns={turns}
              selectedTurnKey={selectedTurnKey}
              showThinking={filters.showThinking}
              onSelect={handleTurnSelect}
              virtuosoRef={virtuoso}
            />
          </div>
          <div
            className="timeline-pane__divider"
            role="slider"
            aria-orientation="vertical"
            aria-label="Resize inspector"
            aria-valuemin={Math.round(MIN_INSPECTOR_FRACTION * 100)}
            aria-valuemax={Math.round(MAX_INSPECTOR_FRACTION * 100)}
            aria-valuenow={Math.round(inspectorFraction * 100)}
            tabIndex={0}
            onMouseDown={onDividerMouseDown}
            onKeyDown={onDividerKeyDown}
          />
          <SessionInspectorPane
            turn={selectedTurn}
            loading={detailPending}
            showThinking={filters.showThinking}
            hideUserPrompt={filters.hiddenSpeakers.has("user")}
            onOpenSubagent={handleSubagent}
            loadMarkdown={loadSelectedMarkdown}
            asOverlay={false}
            focusPairId={focusPairId ?? null}
            focusKey={focusKey ?? null}
            fileTarget={fileTarget}
          />
        </div>
      )}
      {sessionId && (
        <TimelinePromptBar
          sessionId={sessionId}
          session={session}
          onRefresh={refreshSessions}
          turns={turns}
          selectedTurnKey={selectedTurnKey}
          onSelectTurn={handleTurnSelect}
        />
      )}
      {subagents.subagent && (
        <SubagentModal
          subagent={subagents.subagent}
          turns={subagents.turns}
          showThinking={filters.showThinking}
          hideUserPrompt={filters.hiddenSpeakers.has("user")}
          onClose={closeSubagent}
          onOpenSubagent={handleSubagent}
          onBack={subagents.depth > 1 ? backSubagent : undefined}
          fileTarget={fileTarget}
        />
      )}
    </div>
  );
}

/** Attention banner shown while the agent is blocked on a terminal
 * interaction (plan approval, question, permission screen). The timeline
 * can't answer those — point at the terminal and offer the fastest way
 * there for the current display mode. */
function NeedsInputBanner({ session }: { session: SessionView }) {
  const activity = session.activity;
  const isMobile = useMediaQuery(MOBILE_LAYOUT_QUERY);
  const displayMode = useDisplay((store) => store.mode);
  const openTab = useTabs((store) => store.openTab);
  const showTerminal = useCallback(() => {
    if (displayMode === "timeline") {
      useDisplay.getState().togglePeek();
      return;
    }
    openTab({ kind: "terminal", sessionId: session.id }, "top");
  }, [displayMode, openTab, session.id]);

  // A running harness projected as "starting" has not reported a session
  // for this launch: it is on its startup screens, which only the terminal
  // can answer.
  const startingGate =
    session.agent_runtime?.state === "running" && activity?.state === "starting";
  if (
    session.state !== "live" ||
    (!startingGate && activity?.state !== "needs_input" && activity?.state !== "blocked")
  ) {
    return null;
  }
  const label = startingGate
    ? "Agent is starting — answer any startup prompt in the terminal"
    : activity?.state === "blocked"
      ? "Agent reports it is blocked"
      : "Agent needs your input in the terminal";
  return (
    <div className="timeline-pane__attention" role="alert" data-testid="needs-input-banner">
      <Icon name="alert-triangle" size={14} />
      <span className="timeline-pane__attention-label">{label}</span>
      {activity?.summary && (
        <span className="timeline-pane__attention-summary">{activity.summary}</span>
      )}
      {isMobile ? (
        <span className="timeline-pane__attention-guidance">
          Open this session on desktop to answer terminal prompts.
        </span>
      ) : (
        <button
          type="button"
          className="timeline-pane__attention-button"
          onClick={showTerminal}
        >
          {displayMode === "timeline" ? "Peek terminal (⌘⇧E)" : "Go to terminal"}
        </button>
      )}
    </div>
  );
}

function TimelinePromptBar({
  sessionId,
  session,
  onRefresh,
  turns,
  selectedTurnKey,
  onSelectTurn,
}: {
  sessionId: string;
  session?: SessionView;
  onRefresh: () => Promise<void>;
  turns: TurnSummary[];
  selectedTurnKey: string | null;
  onSelectTurn: (key: string) => void;
}) {
  const [text, setText] = useState("");
  const [pending, setPending] = useState<"send" | "interrupt" | AgentLaunchType | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const [pendingPaste, setPendingPaste] = useState<PendingPromptPaste | null>(null);
  const [pasteError, setPasteError] = useState<string | null>(null);
  const runtime = session?.agent_runtime ?? {
    agent: null,
    state: "none",
    started_at: null,
    ended_at: null,
    exit_code: null,
  };
  const metadata = session?.agent_metadata ?? null;
  const live = session?.state === "live";
  const running = live && runtime.state === "running";
  // Whether a prompt lands as a new turn or steers a turn in flight.
  const activityState = session?.activity?.state ?? null;
  const midTurn = running && activityState === "working";
  const idle = running && activityState === "awaiting_prompt";
  const starting = live && runtime.state === "starting";
  const canLaunch = live && !starting && runtime.state !== "running";
  // The gate closes the box while a prompt would be typed into a screen
  // that cannot take it: a harness still on its startup dialogs, or one
  // waiting on a terminal question. "Type anyway" reopens it for the case
  // where the session hook silently failed; the send is still recorded.
  const gate = running ? promptGateFor(activityState) : null;
  const [override, setOverride] = useState(false);
  useEffect(() => {
    if (gate == null) setOverride(false);
  }, [gate]);
  const gated = gate != null && !override;
  const canSend = running && !gated && text.trim().length > 0 && pending == null;
  const canInterrupt = running && pending == null;
  const status = promptStatusText(session ?? null, runtime);
  const meta = promptMetadataText(metadata);
  const unmatchedCount = session?.unmatched_prompt_count ?? 0;
  const openSubmitted = useCallback(
    () => appCommands.openSubmittedPrompts({ sessionId }),
    [sessionId],
  );
  const enableOverride = useCallback(() => setOverride(true), []);

  const startAgent = useCallback(
    async (agent: AgentLaunchType) => {
      if (!canLaunch || pending) return;
      setPending(agent);
      setError(null);
      try {
        await startSessionAgent(sessionId, agent);
        await onRefresh();
      } catch (err) {
        setError(err instanceof Error ? err.message : "failed to start agent");
      } finally {
        setPending(null);
      }
    },
    [canLaunch, onRefresh, pending, sessionId],
  );

  const sendPrompt = useCallback(async () => {
    if (!canSend) return;
    const prompt = promptTextForSend(text);
    setPending("send");
    setError(null);
    try {
      await sendSessionPrompt(sessionId, prompt, override ? { force: true } : undefined);
      setText("");
      await onRefresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : "failed to send prompt");
    } finally {
      setPending(null);
    }
  }, [canSend, onRefresh, override, sessionId, text]);

  const interruptAgent = useCallback(async () => {
    if (!canInterrupt) return;
    setPending("interrupt");
    setError(null);
    try {
      await interruptSessionAgent(sessionId);
      await onRefresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : "failed to interrupt agent");
    } finally {
      setPending(null);
    }
  }, [canInterrupt, onRefresh, sessionId]);

  const onTextChange = useCallback(
    (e: React.ChangeEvent<HTMLTextAreaElement>) => setText(e.target.value),
    [],
  );

  // Same paste model as the terminal: clipboard images and oversized
  // text offer save-as-file under `.sulion-paste/`, inserting the
  // repo-relative path where the caret is.
  const repo = session?.repo ?? null;
  const insertText = useCallback((snippet: string) => {
    setText((prev) => {
      const el = textareaRef.current;
      const start = el?.selectionStart ?? prev.length;
      const end = el?.selectionEnd ?? prev.length;
      return prev.slice(0, start) + snippet + prev.slice(end);
    });
    textareaRef.current?.focus();
  }, []);

  // Library snippets and queued future prompts land here when the
  // timeline is the only projection on screen; otherwise the terminal
  // pane takes them and this box ignores the command.
  const injectionTarget = usePromptInjectionTarget();
  useAppCommand("inject-prompt", ({ sessionId: targetSessionId, text: snippet }) => {
    if (targetSessionId !== sessionId || injectionTarget !== "timeline") return;
    insertText(snippet);
  });

  const onTextPaste = useCallback(
    (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
      if (!e.clipboardData) return;
      setPasteError(null);

      const clipboardImage = imageFromClipboard(e.clipboardData);
      if (clipboardImage) {
        e.preventDefault();
        if (!repo) {
          setPasteError("Clipboard images require a repository-backed session.");
          return;
        }
        setPendingPaste({
          kind: "image",
          file: createClipboardImageUpload(clipboardImage),
          repo,
          sessionId,
          workspaceId: session?.workspace?.id,
        });
        return;
      }

      const raw = e.clipboardData.getData("text/plain");
      const lines = (raw.match(/\n/g)?.length ?? 0) + 1;
      if (repo && (raw.length > PASTE_AS_FILE_BYTES || lines > PASTE_AS_FILE_LINES)) {
        e.preventDefault();
        setPendingPaste({ kind: "text", raw, size: raw.length, lines, repo, sessionId, workspaceId: session?.workspace?.id });
      }
      // Small text falls through to the browser's default insertion.
    },
    [repo, sessionId, session?.workspace?.id],
  );

  const closePaste = useCallback(() => setPendingPaste(null), []);

  const onTextKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        void sendPrompt();
      }
    },
    [sendPrompt],
  );
  const onSendClick = useCallback(() => {
    void sendPrompt();
  }, [sendPrompt]);
  const onInterruptClick = useCallback(() => {
    void interruptAgent();
  }, [interruptAgent]);
  const onStartClaude = useCallback(() => {
    void startAgent("claude");
  }, [startAgent]);
  const onStartCodex = useCallback(() => {
    void startAgent("codex");
  }, [startAgent]);
  const onStartFugu = useCallback(() => {
    void startAgent("fugu");
  }, [startAgent]);

  return (
    <div className="timeline-prompt" aria-label="Agent prompt controls">
      <div className="timeline-prompt__status">
        <span>{status}</span>
        {midTurn && (
          <span
            className="timeline-prompt__activity timeline-prompt__activity--working"
            data-testid="prompt-activity"
          >
            turn in progress — input will steer it
          </span>
        )}
        {idle && (
          <span
            className="timeline-prompt__activity timeline-prompt__activity--idle"
            data-testid="prompt-activity"
          >
            idle — ready for a new prompt
          </span>
        )}
        {meta && <span className="timeline-prompt__meta">{meta}</span>}
        {error && <span className="timeline-prompt__error">{error}</span>}
        {pasteError && <span className="timeline-prompt__error">{pasteError}</span>}
        {live && (
          <Tooltip label="Prompts sent from this box, matched against the transcript">
            <button
              type="button"
              className={`timeline-prompt__submitted${unmatchedCount > 0 ? " timeline-prompt__submitted--open" : ""}`}
              onClick={openSubmitted}
              aria-label="Submitted prompts"
              data-testid="submitted-prompts-button"
            >
              <Icon name="file-text" size={12} />
              {unmatchedCount > 0 ? `${unmatchedCount} unmatched` : "sent"}
            </button>
          </Tooltip>
        )}
        <PromptBarToolbar turns={turns} selectedTurnKey={selectedTurnKey} onSelectTurn={onSelectTurn} />
      </div>
      {running && gate && (
        <div className="timeline-prompt__gate" role="status" data-testid="prompt-gate">
          <Icon name="alert-triangle" size={12} />
          <span>
            {override
              ? "Sending past the gate; the terminal may swallow this text, but it stays in Submitted Prompts."
              : gateText(gate)}
          </span>
          {!override && (
            <button
              type="button"
              className="timeline-prompt__button timeline-prompt__button--gate"
              onClick={enableOverride}
            >
              Type anyway
            </button>
          )}
        </div>
      )}
      {running ? (
        <div className="timeline-prompt__input-row">
          <textarea
            ref={textareaRef}
            value={text}
            onChange={onTextChange}
            onKeyDown={onTextKeyDown}
            onPaste={onTextPaste}
            placeholder={promptPlaceholder(gated, midTurn)}
            rows={2}
            className="timeline-prompt__textarea"
            aria-label="Prompt text"
            disabled={pending != null || gated}
          />
          <button
            type="button"
            className="timeline-prompt__button timeline-prompt__button--primary"
            onClick={onSendClick}
            disabled={!canSend}
          >
            {sendLabel(pending === "send", override, midTurn)}
          </button>
          <Tooltip label="Interrupt running agent (Esc)">
            <button
              type="button"
              className="timeline-prompt__button timeline-prompt__button--icon timeline-prompt__button--interrupt"
              onClick={onInterruptClick}
              disabled={!canInterrupt}
              aria-label="Interrupt agent"
            >
              <Icon name="x" size={16} />
            </button>
          </Tooltip>
        </div>
      ) : (
        <div className="timeline-prompt__launch-row">
          <button
            type="button"
            className="timeline-prompt__button"
            onClick={onStartClaude}
            disabled={!canLaunch || pending != null}
          >
            {pending === "claude" ? "Starting…" : "Start Claude"}
          </button>
          <button
            type="button"
            className="timeline-prompt__button"
            onClick={onStartCodex}
            disabled={!canLaunch || pending != null}
          >
            {pending === "codex" ? "Starting…" : "Start Codex"}
          </button>
          <button
            type="button"
            className="timeline-prompt__button"
            onClick={onStartFugu}
            disabled={!canLaunch || pending != null}
          >
            {pending === "fugu" ? "Starting…" : "Start Fugu"}
          </button>
        </div>
      )}
      {pendingPaste?.sessionId === sessionId && (
        <PasteUploadDialog
          key={pendingPaste.sessionId}
          pending={pendingPaste}
          onInsert={insertText}
          onClose={closePaste}
        />
      )}
    </div>
  );
}

function promptPlaceholder(gated: boolean, midTurn: boolean): string {
  if (gated) return "Input closed until the agent is ready. See the terminal view.";
  if (midTurn) return "Type a steering message. Ctrl+Enter sends into the running turn.";
  return "Type a prompt. Ctrl+Enter sends to the running agent.";
}

function sendLabel(sending: boolean, override: boolean, midTurn: boolean): string {
  if (sending) return "Sending…";
  if (override) return "Send anyway";
  return midTurn ? "Steer" : "Send";
}

/** Trigger row for the two flyouts relocated out of the timeline header:
 * the singleton settings panel (always available) and the turn grid
 * (only when nav mode is "grid" and there's something to navigate). */
function PromptBarToolbar({
  turns,
  selectedTurnKey,
  onSelectTurn,
}: {
  turns: TurnSummary[];
  selectedTurnKey: string | null;
  onSelectTurn: (key: string) => void;
}) {
  const [openFlyout, setOpenFlyout] = useState<"settings" | "grid" | null>(null);
  const settingsTriggerRef = useRef<HTMLButtonElement | null>(null);
  const gridTriggerRef = useRef<HTMLButtonElement | null>(null);
  const [turnNavMode] = useTurnNavMode();
  const closeFlyout = useCallback(() => setOpenFlyout(null), []);
  const toggleSettingsFlyout = useCallback(() => {
    setOpenFlyout((prev) => (prev === "settings" ? null : "settings"));
  }, []);
  const toggleGridFlyout = useCallback(() => {
    setOpenFlyout((prev) => (prev === "grid" ? null : "grid"));
  }, []);
  const showGridTrigger = turnNavMode === "grid" && turns.length > 0;
  useEffect(() => {
    if (!showGridTrigger) setOpenFlyout((prev) => (prev === "grid" ? null : prev));
  }, [showGridTrigger]);

  return (
    <>
      <div className="timeline-prompt__toolbar">
        <Tooltip label="Timeline settings">
          <button
            ref={settingsTriggerRef}
            type="button"
            className="timeline-prompt__button timeline-prompt__button--icon"
            onClick={toggleSettingsFlyout}
            aria-label="Timeline settings"
            aria-pressed={openFlyout === "settings"}
          >
            <Icon name="settings" size={14} />
          </button>
        </Tooltip>
        {showGridTrigger && (
          <Tooltip label="Turn grid">
            <button
              ref={gridTriggerRef}
              type="button"
              className="timeline-prompt__button timeline-prompt__button--icon"
              onClick={toggleGridFlyout}
              aria-label="Turn grid"
              aria-pressed={openFlyout === "grid"}
            >
              <Icon name="layers" size={14} />
            </button>
          </Tooltip>
        )}
      </div>
      {openFlyout === "settings" && (
        <TimelineControlsFlyout anchor={settingsTriggerRef.current} onClose={closeFlyout} />
      )}
      {openFlyout === "grid" && (
        <TurnGridFlyout
          anchor={gridTriggerRef.current}
          turns={turns}
          selectedTurnKey={selectedTurnKey}
          onSelect={onSelectTurn}
          onClose={closeFlyout}
        />
      )}
    </>
  );
}

/** Parked paste in the prompt bar waiting on the user to choose inline
 * vs save-as-file or confirm a clipboard image upload. Mirrors the
 * terminal's paste model. */
type PendingPromptPaste = PendingAttachment;

function promptStatusText(
  session: SessionView | null,
  runtime: NonNullable<SessionView["agent_runtime"]>,
): string {
  if (!session) return "Loading session state…";
  if (session.state !== "live") return `Session is ${session.state}`;
  const agent = runtime.agent ? agentDisplayName(runtime.agent) : "agent";
  switch (runtime.state) {
    case "running":
      return `${agent} is running`;
    case "starting":
      return `${agent} is starting`;
    case "exited": {
      const exitCode = runtime.exit_code == null ? "" : ` (${runtime.exit_code})`;
      return `${agent} exited${exitCode}`;
    }
    case "none":
    default:
      return "No agent running in this PTY";
  }
}

function promptTextForSend(text: string): string {
  return text.replace(/(?:\r\n|\r|\n)+$/, "");
}

function promptMetadataText(metadata: SessionView["agent_metadata"]): string | null {
  if (!metadata) return null;
  const bits = [
    metadata.model,
    metadata.reasoning_effort ? `effort ${metadata.reasoning_effort}` : null,
    metadata.model_provider,
  ].filter(Boolean);
  return bits.length ? bits.join(" · ") : null;
}

function agentDisplayName(agent: string): string {
  if (agent === "claude" || agent === "claude-code") return "Claude";
  if (agent === "codex") return "Codex";
  if (agent === "fugu") return "Fugu";
  return agent;
}

function turnKey(_i: number, t: TurnSummary): string {
  return turnIdentity(t);
}

function turnIdentity(turn: Turn | TurnSummary): string {
  return turn.turn_key ?? `${turn.id}`;
}

function turnSummaryFingerprint(turn: TurnSummary): string {
  return [
    turn.end_timestamp,
    turn.duration_ms,
    turn.event_count,
    turn.operation_count,
    turn.thinking_count,
    turn.has_errors,
  ].join(":");
}

function TurnList({
  turns,
  selectedTurnKey,
  showThinking,
  onSelect,
  virtuosoRef,
}: {
  turns: TurnSummary[];
  selectedTurnKey: string | null;
  showThinking: boolean;
  onSelect: (key: string) => void;
  virtuosoRef: MutableRefObject<VirtuosoHandle | null>;
}) {
  const renderItem = useCallback(
    (_i: number, t: TurnSummary) => (
      <TurnRow
        turn={t}
        selected={selectedTurnKey === turnIdentity(t)}
        showThinking={showThinking}
        onSelect={onSelect}
      />
    ),
    [selectedTurnKey, showThinking, onSelect],
  );
  return (
    <Virtuoso
      ref={virtuosoRef}
      data={turns}
      computeItemKey={turnKey}
      itemContent={renderItem}
      followOutput="smooth"
      className="timeline-pane__virtuoso"
    />
  );
}
