import { useCallback, useMemo } from "react";
import { Virtuoso } from "react-virtuoso";
import type { TimelineSubagent, TimelineTurnSummary } from "../../api/types";
import { Icon } from "../../icons";
import { Overlay } from "../ui";
import type { ToolPair } from "./grouping";
import { useTurnStream } from "./useTurnStream";
import { useTimelineFilters } from "./filters";
import { filterTurn } from "./filterTurn";
import { getTurnDigest } from "../../api/turnStream";
import { refreshTurn } from "../../state/TurnDetailStore";
import type { FileLinkTarget } from "./markdownLinks";
import { TurnDetail } from "./TurnDetail";
import "./SubagentModal.css";

interface Props {
  subagent: TimelineSubagent;
  /** The referenced transcript's turns; null while they load. */
  turns: TimelineTurnSummary[] | null;
  revision?: number;
  active?: boolean;
  error?: string | null;
  showThinking: boolean;
  hideUserPrompt?: boolean;
  onClose: () => void;
  /** Drill into a nested Task pair's subagent. */
  onOpenSubagent?: (pair: ToolPair) => void;
  /** Present when a nested subagent is shown; returns to its parent. */
  onBack?: () => void;
  /** Repo the subagent's relative markdown links open files from. */
  fileTarget?: FileLinkTarget | null;
}

export function SubagentModal({
  subagent,
  turns,
  revision = 0,
  active = true,
  error,
  showThinking,
  hideUserPrompt = false,
  onClose,
  onOpenSubagent,
  onBack,
  fileTarget = null,
}: Props) {
  const renderTurn = useCallback((_index: number, turn: TimelineTurnSummary) => (
    <SubagentTurn summary={turn} session={subagent.session_uuid!} revision={revision} active={active}
      showThinking={showThinking} hideUserPrompt={hideUserPrompt}
      onOpenSubagent={onOpenSubagent} fileTarget={fileTarget} />
  ), [subagent.session_uuid, revision, active, showThinking, hideUserPrompt, onOpenSubagent, fileTarget]);
  const eventCount = turns
    ? turns.reduce((sum, turn) => sum + turn.event_count, 0)
    : subagent.event_count;
  const turnCount = turns ? turns.length : subagent.turn_count;
  const subtitle =
    `${eventCount} events · ` + `${turnCount} turn${turnCount === 1 ? "" : "s"}`;

  return (
    <Overlay
      open
      onClose={onClose}
      modal
      title={subagent.title}
      subtitle={subtitle}
      leading={<Icon name="parent-session" size={16} />}
      width={760}
      maxHeight="78vh"
      className="sm"
      data-testid="subagent-modal"
    >
      {onBack && (
        <button type="button" className="sm__back" onClick={onBack}>
          <Icon name="arrow-left" size={14} /> parent agent
        </button>
      )}
      {error && <div role="alert">{error}</div>}
      {turns == null && !error && <div className="sm__empty">Loading subagent turns…</div>}
      {turns?.length === 0 && (
        <div className="sm__empty">
          No subagent events found for this Task. The subagent may not have
          emitted yet.
        </div>
      )}
      {turns && (turns.length > 8
        ? <Virtuoso className="sm__turn-list" data={turns} itemContent={renderTurn} />
        : turns.map((turn, index) => <div key={turn.id}>{renderTurn(index, turn)}</div>))}
    </Overlay>
  );
}

function SubagentTurn({ summary, session, revision, active, ...props }: {
  summary: TimelineTurnSummary; session: string; revision: number; active: boolean;
  showThinking: boolean; hideUserPrompt: boolean;
  onOpenSubagent?: (pair: ToolPair) => void; fileTarget: FileLinkTarget | null;
}) {
  const detail = useTurnStream(session, summary.id, revision, active);
  const { filters } = useTimelineFilters();
  const turn = useMemo(() => detail?.turn ? filterTurn(detail.turn, filters) : null, [detail?.turn, filters]);
  const loadMarkdown = useCallback(() => getTurnDigest(session, summary.id), [session, summary.id]);
  const retry = useCallback(() => refreshTurn(session, summary.id, revision, true), [session, summary.id, revision]);
  return <div className="sm__turn">
    {detail?.error && <div role="alert">{detail.error}
      <button type="button" onClick={retry}>Retry</button>
    </div>}
    {turn ? <TurnDetail turn={turn} loadMarkdown={loadMarkdown} {...props} /> : <p>Loading turn detail…</p>}
  </div>;
}
