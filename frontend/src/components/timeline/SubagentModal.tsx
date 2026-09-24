import type { TimelineSubagent } from "../../api/types";
import { Icon } from "../../icons";
import { Overlay } from "../ui";
import type { ToolPair, Turn } from "./grouping";
import type { FileLinkTarget } from "./markdownLinks";
import { TurnDetail } from "./TurnDetail";
import "./SubagentModal.css";

interface Props {
  subagent: TimelineSubagent;
  /** The referenced transcript's turns; null while they load. */
  turns: Turn[] | null;
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
  showThinking,
  hideUserPrompt = false,
  onClose,
  onOpenSubagent,
  onBack,
  fileTarget = null,
}: Props) {
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
      {turns == null && <div className="sm__empty">Loading subagent turns…</div>}
      {turns?.length === 0 && (
        <div className="sm__empty">
          No subagent events found for this Task. The subagent may not have
          emitted yet.
        </div>
      )}
      {turns?.map((turn) => (
        <div key={turn.id} className="sm__turn">
          <TurnDetail
            turn={turn}
            showThinking={showThinking}
            hideUserPrompt={hideUserPrompt}
            onOpenSubagent={onOpenSubagent}
            fileTarget={fileTarget}
          />
        </div>
      ))}
    </Overlay>
  );
}
