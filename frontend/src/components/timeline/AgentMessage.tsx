import { useCallback, useState, type SyntheticEvent } from "react";
import type { TimelineChunk } from "../../api/types";
import { Markdown } from "./Markdown";
import type { FileLinkTarget } from "./markdownLinks";

export function AgentMessage({ message, fileTarget }: {
  message: Extract<TimelineChunk, { kind: "agent_message" }>;
  fileTarget: FileLinkTarget | null;
}) {
  const [open, setOpen] = useState(false);
  const onToggle = useCallback((event: SyntheticEvent<HTMLDetailsElement>) => {
    setOpen(event.currentTarget.open);
  }, []);
  const label = message.message_type === "FINAL_ANSWER" ? "Agent reply" : "Agent message";
  return (
    <details className="td__sub td__agent-message" onToggle={onToggle}>
      <summary>
        <span className="td__agent-message-label">{label}</span>{" "}
        <span className="td__agent-message-route">{message.author} → {message.recipient}</span>
        {message.message_type && !["MESSAGE", "FINAL_ANSWER"].includes(message.message_type) && (
          <span className="td__sub-label"> {message.message_type}</span>
        )}
      </summary>
      {open && <div className="td__agent-message-body">
        <Markdown source={message.text || "Message content unavailable."} fileTarget={fileTarget} />
      </div>}
    </details>
  );
}
