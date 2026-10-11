import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { SessionView } from "../api/types";
import { SessionCreationDialog } from "./SessionCreationDialog";

describe("SessionCreationDialog", () => {
  it("retains launch choices after failure and prevents duplicate launches while pending", async () => {
    let rejectLaunch: (reason: Error) => void = () => {};
    const createSession = vi.fn(
      () =>
        new Promise<SessionView>((_resolve, reject) => {
          rejectLaunch = reject;
        }),
    );
    const onClose = vi.fn();
    render(
      <SessionCreationDialog
        repos={[{ name: "alpha", path: "/tmp/alpha" }]}
        metaRepos={[]}
        createSession={createSession}
        onCreated={vi.fn()}
        onClose={onClose}
      />,
    );
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "alpha Repository" }));
    await user.selectOptions(screen.getByLabelText("workspace mode"), "main");
    await user.type(
      screen.getByLabelText("working directory"),
      "/tmp/alpha/src",
    );
    await user.selectOptions(screen.getByLabelText("launch agent"), "fugu");
    await user.click(screen.getByRole("button", { name: "Spawn" }));
    expect(createSession).toHaveBeenCalledTimes(1);
    expect(createSession).toHaveBeenCalledWith({
      repo: "alpha",
      workspace_mode: "main",
      working_dir: "/tmp/alpha/src",
      launch_agent: "fugu",
    });
    expect(
      (screen.getByRole("button", { name: "Starting…" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    await user.click(screen.getByRole("button", { name: "Starting…" }));
    await user.keyboard("{Escape}");
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => rejectLaunch(new Error("Node unavailable")));
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toBe("Node unavailable"),
    );
    expect(
      (screen.getByLabelText("working directory") as HTMLInputElement).value,
    ).toBe("/tmp/alpha/src");
    expect(
      (screen.getByLabelText("launch agent") as HTMLSelectElement).value,
    ).toBe("fugu");
    expect(createSession).toHaveBeenCalledTimes(1);
    expect(
      (screen.getByRole("button", { name: "Spawn" }) as HTMLButtonElement)
        .disabled,
    ).toBe(false);
  });

  it("disables unavailable repositories", () => {
    render(
      <SessionCreationDialog
        repos={[{ name: "missing", path: "/tmp/missing", exists: false }]}
        metaRepos={[]}
        createSession={vi.fn()}
        onCreated={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    expect(
      (
        screen.getByRole("button", {
          name: /missing Repository/,
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
  });
});
