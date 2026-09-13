import type { PromptGate, SessionActivityState } from "../../api/types";

/** The projected activity states in which a timeline prompt would be typed
 * into a screen that cannot take it. Mirrors `submitted_prompts::prompt_gate`
 * on the backend, which enforces the same rule on the prompt route. */
export function promptGateFor(state: SessionActivityState | null): PromptGate | null {
  switch (state) {
    case "starting":
      return "starting";
    case "needs_input":
      return "needs_input";
    case "blocked":
      return "blocked";
    default:
      return null;
  }
}

export function gateText(gate: PromptGate): string {
  switch (gate) {
    case "starting":
      return "The agent is still starting. Answer any startup prompt in the terminal view; a prompt sent now would be typed into it.";
    case "needs_input":
      return "The agent is waiting for input in the terminal. A prompt sent now would land in that dialog.";
    case "blocked":
      return "The agent reports it is blocked. Check the terminal before sending.";
  }
}
