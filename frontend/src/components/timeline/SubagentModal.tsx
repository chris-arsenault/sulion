import type { TimelineSubagent } from "../../api/types";
import { Icon } from "../../icons";
import { Overlay } from "../ui";
import type { ToolPair } from "./grouping";
import { TurnDetail } from "./TurnDetail";
import "./SubagentModal.css";

interface Props {
  subagent: TimelineSubagent;
  showThinking: boolean;
  onClose: () => void;
  /** Drill into a nested Task pair's subagent. */
  onOpenSubagent?: (pair: ToolPair) => void;
  /** Present when a nested subagent is shown; returns to its parent. */
  onBack?: () => void;
}

export function SubagentModal({
  subagent,
  showThinking,
  onClose,
  onOpenSubagent,
  onBack,
}: Props) {
  const subtitle =
    `${subagent.event_count} events · ` +
    `${subagent.turns.length} turn${subagent.turns.length === 1 ? "" : "s"}`;

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
      {subagent.turns.length === 0 && (
        <div className="sm__empty">
          No subagent events found for this Task. The subagent may not have
          emitted yet.
        </div>
      )}
      {subagent.turns.map((turn) => (
        <div key={turn.id} className="sm__turn">
          <TurnDetail
            turn={turn}
            showThinking={showThinking}
            onOpenSubagent={onOpenSubagent}
          />
        </div>
      ))}
    </Overlay>
  );
}
