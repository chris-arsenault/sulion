import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import * as apiClient from "../api/client";
import { appCommands, subscribeToAppCommands } from "../state/AppCommands";
import { useTabStore } from "../state/TabStore";
import { LibraryPanel } from "./LibraryPanel";

const DEPLOY_PROMPT_NAME = "Deploy prompt";

describe("LibraryPanel", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("refreshes prompts when the matching library-changed command is emitted", async () => {
    const listLibrary = vi.spyOn(apiClient, "listLibrary");
    listLibrary
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([
        {
          slug: "deploy",
          name: DEPLOY_PROMPT_NAME,
          created_at: null,
          updated_at: null,
          body: "first body",
        },
      ])
      .mockResolvedValueOnce([
        {
          slug: "ship-it",
          name: "Ship prompt",
          created_at: null,
          updated_at: null,
          body: "second body",
        },
      ]);

    render(<LibraryPanel />);

    await waitFor(() => expect(screen.getByText(DEPLOY_PROMPT_NAME)).toBeDefined());

    appCommands.libraryChanged({ kind: "prompts" });

    await waitFor(() => expect(screen.getByText("Ship prompt")).toBeDefined());
  });

  it("injects a prompt into the active terminal on click", async () => {
    vi.spyOn(apiClient, "listLibrary")
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([
        {
          slug: "deploy",
          name: DEPLOY_PROMPT_NAME,
          created_at: null,
          updated_at: null,
          body: "echo deploy\r\nnow",
        },
      ]);
    useTabStore.getState().openTab({ kind: "terminal", sessionId: "sess-1" }, "top");

    const seen: Array<unknown> = [];
    const unsubscribe = subscribeToAppCommands((command) => {
      seen.push(command);
    });

    render(<LibraryPanel />);
    const user = userEvent.setup();

    await waitFor(() => expect(screen.getByText(DEPLOY_PROMPT_NAME)).toBeDefined());
    await user.click(screen.getByText(DEPLOY_PROMPT_NAME));

    expect(seen).toContainEqual({
      type: "inject-terminal",
      sessionId: "sess-1",
      text: "echo deploy\r\nnow",
    });
    unsubscribe();
  });

  it("opens a reference tab on click", async () => {
    vi.spyOn(apiClient, "listLibrary")
      .mockResolvedValueOnce([
        {
          slug: "ticket-order",
          name: "Ticket order",
          created_at: null,
          updated_at: null,
          body: "43, 48, 49",
        },
      ])
      .mockResolvedValueOnce([]);

    render(<LibraryPanel />);
    const user = userEvent.setup();

    await waitFor(() => expect(screen.getByText("Ticket order")).toBeDefined());
    await user.click(screen.getByText("Ticket order"));

    expect(
      Object.values(useTabStore.getState().tabs).some(
        (tab) => tab.kind === "ref" && tab.slug === "ticket-order",
      ),
    ).toBe(true);
  });
});
