import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";

import * as apiClient from "../api/client";
import { appCommands } from "../state/AppCommands";
import { LibrarySection } from "./LibrarySection";

describe("LibrarySection", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("refreshes when the matching library-changed command is emitted", async () => {
    const listLibrary = vi.spyOn(apiClient, "listLibrary");
    listLibrary
      .mockResolvedValueOnce([
        {
          slug: "deploy",
          name: "Deploy prompt",
          tags: [],
          created_at: null,
          body: "first",
          extras: {},
        },
      ])
      .mockResolvedValueOnce([
        {
          slug: "ship-it",
          name: "Ship prompt",
          tags: [],
          created_at: null,
          body: "second",
          extras: {},
        },
      ]);

    render(
      <LibrarySection
        repo="alpha"
        kind="prompts"
        open={true}
        onToggle={() => {}}
      />,
    );

    await waitFor(() => expect(screen.getByText("Deploy prompt")).toBeDefined());

    appCommands.libraryChanged({ repo: "alpha", kind: "prompts" });

    await waitFor(() => expect(screen.getByText("Ship prompt")).toBeDefined());
    expect(listLibrary).toHaveBeenCalledTimes(2);
  });
});
