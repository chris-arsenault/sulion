import { afterEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
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

  it("starts the reference and prompt sections collapsed", () => {
    vi.spyOn(apiClient, "listLibrary").mockImplementation(
      () => new Promise<Awaited<ReturnType<typeof apiClient.listLibrary>>>(() => {}),
    );

    render(<LibraryPanel />);

    expect(
      screen.getByRole("button", { name: /References/ }).getAttribute("aria-expanded"),
    ).toBe("false");
    expect(
      screen.getByRole("button", { name: /Prompts/ }).getAttribute("aria-expanded"),
    ).toBe("false");
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
    await waitFor(() => expect(listLibrary).toHaveBeenCalledTimes(2));

    act(() => appCommands.libraryChanged({ kind: "prompts" }));

    await waitFor(() => expect(screen.getByText("Ship prompt")).toBeDefined());
    expect(
      screen.getByRole("button", { name: /Prompts/ }).getAttribute("aria-expanded"),
    ).toBe("true");
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
    await user.click(screen.getByRole("button", { name: /Prompts/ }));

    await waitFor(() => expect(screen.getByText(DEPLOY_PROMPT_NAME)).toBeDefined());
    await user.click(screen.getByText(DEPLOY_PROMPT_NAME));

    expect(seen).toContainEqual({
      type: "inject-terminal",
      sessionId: "sess-1",
      text: "echo deploy\r\nnow",
    });
    unsubscribe();
  });

  it("asks for template values before injecting a prompt with variables", async () => {
    vi.spyOn(apiClient, "listLibrary")
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([
        {
          slug: "deploy",
          name: DEPLOY_PROMPT_NAME,
          created_at: null,
          updated_at: null,
          body: "do item $n in $repo. cost $$5. repeat $n",
        },
      ]);
    useTabStore.getState().openTab({ kind: "terminal", sessionId: "sess-1" }, "top");

    const seen: Array<unknown> = [];
    const unsubscribe = subscribeToAppCommands((command) => {
      seen.push(command);
    });

    render(<LibraryPanel />);
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: /Prompts/ }));

    await waitFor(() => expect(screen.getByText(DEPLOY_PROMPT_NAME)).toBeDefined());
    await user.click(screen.getByText(DEPLOY_PROMPT_NAME));

    expect(screen.getByText("Prompt Values")).toBeDefined();
    expect(seen).toHaveLength(0);

    await user.type(screen.getByLabelText("Value for n"), "42");
    await user.type(screen.getByLabelText("Value for repo"), "sulion");
    await user.click(screen.getByRole("button", { name: "Send" }));

    expect(seen).toContainEqual({
      type: "inject-terminal",
      sessionId: "sess-1",
      text: "do item 42 in sulion. cost $5. repeat 42",
    });
    expect(screen.queryByText("Prompt Values")).toBeNull();
    unsubscribe();
  });

  it("uses a newly saved uppercase template variable before refresh completes", async () => {
    const listLibrary = vi.spyOn(apiClient, "listLibrary");
    listLibrary
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([])
      .mockImplementation(
        () => new Promise<Awaited<ReturnType<typeof apiClient.listLibrary>>>(() => {}),
      );
    vi.spyOn(apiClient, "saveLibraryEntry").mockResolvedValue({
      slug: "refactor",
      name: "refactor",
      created_at: null,
      updated_at: "2026-05-28T03:15:29Z",
      body: "Implement item $N.",
    });
    useTabStore.getState().openTab({ kind: "terminal", sessionId: "sess-1" }, "top");

    render(<LibraryPanel />);
    const user = userEvent.setup();

    await waitFor(() => expect(listLibrary).toHaveBeenCalledTimes(2));
    await user.click(screen.getByLabelText("New prompt"));
    await user.type(screen.getByLabelText("Prompt name"), "refactor");
    await user.type(screen.getByLabelText("Prompt body"), "Implement item $N.");
    await user.click(screen.getByRole("button", { name: "Save prompt" }));
    await waitFor(() =>
      expect(apiClient.saveLibraryEntry).toHaveBeenCalledWith(
        "prompts",
        { name: "refactor", body: "Implement item $N." },
        undefined,
      ),
    );

    await user.click(screen.getByText("refactor"));

    expect(screen.getByText("Prompt Values")).toBeDefined();
    expect(screen.getByLabelText("Value for N")).toBeDefined();
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
    await user.click(screen.getByRole("button", { name: /References/ }));

    await waitFor(() => expect(screen.getByText("Ticket order")).toBeDefined());
    await user.click(screen.getByText("Ticket order"));

    expect(
      Object.values(useTabStore.getState().tabs).some(
        (tab) => tab.kind === "ref" && tab.slug === "ticket-order",
      ),
    ).toBe(true);
  });
});
