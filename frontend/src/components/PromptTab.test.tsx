import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import * as apiClient from "../api/client";
import { subscribeToAppCommands } from "../state/AppCommands";
import { useTabStore } from "../state/TabStore";
import { PromptTab } from "./PromptTab";

describe("PromptTab", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("injects the prompt body into the active terminal through app commands", async () => {
    vi.spyOn(apiClient, "getLibraryEntry").mockResolvedValue({
      slug: "deploy",
      name: "Deploy prompt",
      tags: [],
      created_at: null,
      body: "echo deploy\r\nnow",
      extras: {},
    });
    useTabStore.getState().openTab({ kind: "terminal", sessionId: "sess-1" }, "top");

    const seen: Array<unknown> = [];
    const unsubscribe = subscribeToAppCommands((command) => {
      seen.push(command);
    });

    render(<PromptTab repo="alpha" slug="deploy" />);
    const user = userEvent.setup();

    await waitFor(() =>
      expect(screen.getByRole("button", { name: /inject into terminal/i })).toBeDefined(),
    );
    await user.click(screen.getByRole("button", { name: /inject into terminal/i }));

    expect(seen).toContainEqual({
      type: "inject-terminal",
      sessionId: "sess-1",
      text: "echo deploy\r\nnow",
    });
    unsubscribe();
  });
});
