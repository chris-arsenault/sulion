import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { FileTracePanel } from "./FileTracePanel";
import { resetTabStore, useTabStore } from "../state/TabStore";
import { ContextMenuHost } from "./common/ContextMenu";
import { appCommands } from "../state/AppCommands";

const TURN_PREVIEW = "Update lib.rs";

describe("FileTracePanel", () => {
  beforeEach(() => {
    resetTabStore();
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo) => {
        const url = new URL(
          typeof input === "string" ? input : (input as Request).url,
          "http://localhost",
        );
        if (url.pathname === "/api/repos/alpha/file-trace") {
          return new Response(
            JSON.stringify({
              path: "src/lib.rs",
              dirty: " M",
              current_diff: { additions: 4, deletions: 1 },
              touches: [
                {
                  pty_session_id: "11111111-1111-1111-1111-111111111111",
                  session_uuid: "22222222-2222-2222-2222-222222222222",
                  session_agent: "codex",
                  session_label: "investigation",
                  session_state: "dead",
                  turn_id: 42,
                  turn_preview: TURN_PREVIEW,
                  turn_timestamp: "2026-04-19T00:00:00Z",
                  operation_type: "edit",
                  operation_category: "create_content",
                  touch_kind: "write",
                  is_write: true,
                },
              ],
            }),
            {
              status: 200,
              headers: { "Content-Type": "application/json" },
            },
          );
        }
        return new Response("", { status: 404 });
      }),
    );
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("opens the related timeline turn with a focus target", async () => {
    render(
      <>
        <FileTracePanel repo="alpha" path="src/lib.rs" />
        <ContextMenuHost />
      </>,
    );

    await waitFor(() => {
      expect(screen.getByText(TURN_PREVIEW)).toBeDefined();
    });

    const user = userEvent.setup();
    await user.click(screen.getByText(TURN_PREVIEW));

    const tabs = useTabStore.getState().tabs;
    const timelineTab = Object.values(tabs).find((tab) => tab.kind === "timeline");
    expect(timelineTab?.sessionId).toBe("11111111-1111-1111-1111-111111111111");
    expect(timelineTab?.focusTurnId).toBe(42);
    expect(typeof timelineTab?.focusKey).toBe("string");
  });

  it("opens file actions from the row context menu", async () => {
    const openFileSpy = vi.spyOn(appCommands, "openFile");

    render(
      <>
        <FileTracePanel repo="alpha" path="src/lib.rs" />
        <ContextMenuHost />
      </>,
    );

    await waitFor(() => {
      expect(screen.getByText(TURN_PREVIEW)).toBeDefined();
    });

    const user = userEvent.setup();
    await user.pointer({
      keys: "[MouseRight]",
      target: screen.getByText(TURN_PREVIEW),
    });
    await user.click(screen.getByRole("menuitem", { name: /open file/i }));

    expect(openFileSpy).toHaveBeenCalledWith({ repo: "alpha", path: "src/lib.rs" });
  });
});
