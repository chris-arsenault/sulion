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
  uploadRepoFile,
} from "../api/client";
import { ConfirmDialog } from "./common/ConfirmDialog";
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
  TimelineSubagent,
  TimelineSummaryResponse,
} from "../api/types";
import { useMediaQuery } from "../hooks/useMediaQuery";
import { MOBILE_LAYOUT_QUERY } from "../state/displayPolicy";
import { useTimelineFontScale, useTurnNavMode } from "../state/paneTextScale";
import { useSessions } from "../state/SessionStore";
import { useTabs } from "../state/TabStore";
import { useDisplay } from "../state/DisplayStore";
import { useTimelineFilters } from "./timeline/filters";
import { type ToolPair, type Turn, type TurnSummary } from "./timeline/grouping";
import { SessionInspectorPane } from "./timeline/SessionInspectorPane";
import { SubagentModal } from "./timeline/SubagentModal";
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

interface CachedTurnDetail {
  fingerprint: string;
  /** Resource revision at fetch time. While the subagent modal is open the
   * detail refetches on every revision tick, because subagent (sidechain)
   * events don't move the parent turn's summary fingerprint. */
  revision: number;
  turn: Turn;
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
  // Pair ids from the selected turn down through nested Task pairs. The
  // subagent shown in the modal is re-derived from the (live) detail cache
  // on every render, so it updates as the subagent emits events.
  const [subagentPath, setSubagentPath] = useState<string[]>([]);
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

  useEffect(() => {
    setTimeline(null);
    setCurrentSessionUuid(null);
    setCurrentSessionAgent(null);
    setLoadError(null);
    setDetailError(null);
    setDetailCache(new Map());
    setSubagentPath([]);
    setSelectedTurnKey(null);
    appliedFocusKeyRef.current = null;
    loadedSummaryKeyRef.current = null;
  }, [sessionId, repo]);

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

  const subagentOpen = subagentPath.length > 0;
  useEffect(() => {
    if (!active || !selectedSummary || !selectedTurnKey) return;
    if (selectedFingerprint == null) return;
    const cached = detailCache.get(selectedTurnKey);
    const cacheFresh =
      cached?.fingerprint === selectedFingerprint &&
      (!subagentOpen || cached.revision === resourceRevision);
    if (cacheFresh) return;
    if (!sessionId && (!repo || !selectedSummary.session_uuid)) return;

    let cancelled = false;
    const fetchDetail = async () => {
      try {
        const resp = sessionId
          ? await getTimelineTurn(sessionId, selectedSummary.id, query)
          : await getRepoTimelineTurn(
              repo!,
              selectedSummary.session_uuid!,
              selectedSummary.id,
              query,
            );
        if (cancelled) return;
        setDetailCache((prev) => {
          const entry = prev.get(selectedTurnKey);
          if (
            entry?.fingerprint === selectedFingerprint &&
            entry.revision === resourceRevision
          ) {
            return prev;
          }
          const next = new Map(prev);
          next.set(selectedTurnKey, {
            fingerprint: selectedFingerprint,
            revision: resourceRevision,
            turn: resp.turn,
          });
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
    query,
    repo,
    resourceRevision,
    selectedFingerprint,
    selectedSummary,
    selectedTurnKey,
    sessionId,
    subagentOpen,
  ]);

  // Resolve the open subagent by walking pair ids from the selected turn
  // through nested Task pairs. Derived (not stored) so a detail refetch
  // refreshes the modal in place.
  const subagent = useMemo<TimelineSubagent | null>(() => {
    if (!selectedTurn || subagentPath.length === 0) return null;
    let pairs = selectedTurn.tool_pairs;
    let current: TimelineSubagent | null = null;
    for (const pairId of subagentPath) {
      current = pairs.find((pair) => pair.id === pairId)?.subagent ?? null;
      if (!current) return null;
      pairs = current.turns.flatMap((turn) => turn.tool_pairs);
    }
    return current;
  }, [selectedTurn, subagentPath]);

  const handleSubagent = useCallback((pair: ToolPair) => {
    if (pair.subagent) setSubagentPath((prev) => [...prev, pair.id]);
  }, []);
  const closeSubagent = useCallback(() => setSubagentPath([]), []);
  const backSubagent = useCallback(
    () => setSubagentPath((prev) => prev.slice(0, -1)),
    [],
  );

  // The path can stop resolving when a filter change or refetch drops the
  // pair it pointed at; drop it rather than let later opens append to it.
  useEffect(() => {
    if (subagentPath.length > 0 && selectedTurn && !subagent) {
      setSubagentPath([]);
    }
  }, [subagent, selectedTurn, subagentPath.length]);

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
      setSubagentPath([]);
      if (tabId) clearTimelineFocus(tabId);
      if (filters.followLatest) setFollowLatest(false);
    },
    [tabId, clearTimelineFocus, filters.followLatest, setFollowLatest],
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
      {sessionId && session && <NeedsInputBanner session={session} />}
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
              asOverlay={false}
              focusPairId={focusPairId ?? null}
              focusKey={focusKey ?? null}
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
                asOverlay
                onClose={clearSelectedTurn}
                focusPairId={focusPairId ?? null}
                focusKey={focusKey ?? null}
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
            asOverlay={false}
            focusPairId={focusPairId ?? null}
            focusKey={focusKey ?? null}
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
            asOverlay={false}
            focusPairId={focusPairId ?? null}
            focusKey={focusKey ?? null}
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
      {subagent && (
        <SubagentModal
          subagent={subagent}
          showThinking={filters.showThinking}
          hideUserPrompt={filters.hiddenSpeakers.has("user")}
          onClose={closeSubagent}
          onOpenSubagent={handleSubagent}
          onBack={subagentPath.length > 1 ? backSubagent : undefined}
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

  if (
    session.state !== "live" ||
    (activity?.state !== "needs_input" && activity?.state !== "blocked")
  ) {
    return null;
  }
  const label =
    activity.state === "blocked"
      ? "Agent reports it is blocked"
      : "Agent needs your input in the terminal";
  return (
    <div className="timeline-pane__attention" role="alert" data-testid="needs-input-banner">
      <Icon name="alert-triangle" size={14} />
      <span className="timeline-pane__attention-label">{label}</span>
      {activity.summary && (
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
  const canSend = running && text.trim().length > 0 && pending == null;
  const canInterrupt = running && pending == null;
  const status = promptStatusText(session ?? null, runtime);
  const meta = promptMetadataText(metadata);

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
      await sendSessionPrompt(sessionId, prompt);
      setText("");
      await onRefresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : "failed to send prompt");
    } finally {
      setPending(null);
    }
  }, [canSend, onRefresh, sessionId, text]);

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
        });
        return;
      }

      const raw = e.clipboardData.getData("text/plain");
      const lines = (raw.match(/\n/g)?.length ?? 0) + 1;
      if (repo && (raw.length > PASTE_AS_FILE_BYTES || lines > PASTE_AS_FILE_LINES)) {
        e.preventDefault();
        setPendingPaste({ kind: "text", raw, size: raw.length, lines, repo });
      }
      // Small text falls through to the browser's default insertion.
    },
    [repo],
  );

  const acceptPasteAsFile = useCallback(async () => {
    const parked = pendingPaste;
    if (!parked) return;
    setPendingPaste(null);
    setPasteError(null);
    const file =
      parked.kind === "image"
        ? parked.file
        : new File(
            [parked.raw],
            `paste-${new Date().toISOString().replace(/[:.]/g, "-").replace("T", "_")}.txt`,
            { type: "text/plain" },
          );
    try {
      const res = await uploadRepoFile(parked.repo, ".sulion-paste", file);
      insertText(res.path + " ");
    } catch (err) {
      // Text still has an inline fallback; an image cannot be
      // represented in the textarea, so keep its failure visible.
      if (parked.kind === "text") {
        insertText(parked.raw);
      } else {
        setPasteError(
          err instanceof Error
            ? `Clipboard image upload failed: ${err.message}`
            : "Clipboard image upload failed.",
        );
      }
    }
  }, [insertText, pendingPaste]);

  const cancelPendingPaste = useCallback(() => {
    const parked = pendingPaste;
    if (!parked) return;
    setPendingPaste(null);
    if (parked.kind === "text") {
      insertText(parked.raw);
    }
  }, [insertText, pendingPaste]);
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
        <PromptBarToolbar turns={turns} selectedTurnKey={selectedTurnKey} onSelectTurn={onSelectTurn} />
      </div>
      {running ? (
        <div className="timeline-prompt__input-row">
          <textarea
            ref={textareaRef}
            value={text}
            onChange={onTextChange}
            onKeyDown={onTextKeyDown}
            onPaste={onTextPaste}
            placeholder={
              midTurn
                ? "Type a steering message. Ctrl+Enter sends into the running turn."
                : "Type a prompt. Ctrl+Enter sends to the running agent."
            }
            rows={2}
            className="timeline-prompt__textarea"
            aria-label="Prompt text"
            disabled={pending != null}
          />
          <button
            type="button"
            className="timeline-prompt__button timeline-prompt__button--primary"
            onClick={onSendClick}
            disabled={!canSend}
          >
            {pending === "send" ? "Sending…" : midTurn ? "Steer" : "Send"}
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
      {pendingPaste && (
        <ConfirmDialog
          title={pendingPaste.kind === "text" ? "Large paste" : "Clipboard image"}
          message={
            pendingPaste.kind === "text"
              ? `Clipboard is ${pendingPaste.size} bytes / ${pendingPaste.lines} lines. ` +
                "Save it to .sulion-paste/ and insert the path, or paste the raw contents inline?"
              : `Upload ${pendingPaste.file.name} (${pendingPaste.file.size} bytes) to ` +
                ".sulion-paste/ and insert its path?"
          }
          confirmLabel={pendingPaste.kind === "text" ? "Save as file" : "Upload image"}
          cancelLabel={pendingPaste.kind === "text" ? "Paste inline" : "Cancel"}
          onConfirm={acceptPasteAsFile}
          onCancel={cancelPendingPaste}
        />
      )}
    </div>
  );
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
type PendingPromptPaste =
  | { kind: "text"; raw: string; size: number; lines: number; repo: string }
  | { kind: "image"; file: File; repo: string };

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
