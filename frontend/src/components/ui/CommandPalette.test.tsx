import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { CommandPalette, type PaletteCommand } from "./CommandPalette";

function commands(overrides?: Partial<PaletteCommand>[]): PaletteCommand[] {
  const base: PaletteCommand[] = [
    { id: "action.overview", label: "Open overview", run: vi.fn() },
    { id: "action.pin", label: "Toggle sidebar pin", run: vi.fn() },
    {
      id: "repo.alpha",
      label: "Jump to repo · alpha",
      searchOnly: true,
      rank: 20,
      run: vi.fn(),
    },
    {
      id: "session.a",
      label: "Open session · alpha / batcher fix",
      searchOnly: true,
      rank: 30,
      run: vi.fn(),
    },
    {
      id: "plan.a",
      label: "Open plan · alpha / durable ingest",
      searchOnly: true,
      rank: 10,
      run: vi.fn(),
    },
  ];
  if (!overrides) return base;
  return base.map((cmd, i) => ({ ...cmd, ...overrides[i] }));
}

describe("CommandPalette", () => {
  it("shows only action commands while the query is empty", () => {
    render(
      <CommandPalette open onClose={vi.fn()} commands={commands()} />,
    );
    expect(screen.getByText("Open overview")).toBeDefined();
    expect(screen.getByText("Toggle sidebar pin")).toBeDefined();
    expect(screen.queryByText(/jump to repo/i)).toBeNull();
    expect(screen.queryByText(/open session/i)).toBeNull();
    expect(screen.queryByText(/open plan/i)).toBeNull();
  });

  it("surfaces searchOnly entries once the user types, rank-ordered on ties", async () => {
    render(
      <CommandPalette open onClose={vi.fn()} commands={commands()} />,
    );
    const user = userEvent.setup();
    await user.type(screen.getByRole("textbox"), "alpha");

    const options = screen.getAllByRole("option").map((el) => el.textContent);
    expect(options).toHaveLength(3);
    // Equal match scores → rank decides: session (30) > repo (20) > plan (10).
    expect(options[0]).toContain("Open session");
    expect(options[1]).toContain("Jump to repo");
    expect(options[2]).toContain("Open plan");
  });

  it("runs the top match on Enter", async () => {
    const cmds = commands();
    const onClose = vi.fn();
    render(<CommandPalette open onClose={onClose} commands={cmds} />);
    const user = userEvent.setup();
    await user.type(screen.getByRole("textbox"), "batcher fix");
    await user.keyboard("{Enter}");
    expect(onClose).toHaveBeenCalled();
    await Promise.resolve();
    expect(cmds[3]!.run).toHaveBeenCalled();
  });
});
