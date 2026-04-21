import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { ConfirmDialog } from "./ConfirmDialog";

const noop = () => {};

describe("ConfirmDialog", () => {
  it("renders title, message, and buttons", () => {
    render(
      <ConfirmDialog
        title="Delete?"
        message="Are you sure"
        onConfirm={noop}
        onCancel={noop}
      />,
    );
    expect(screen.getByText("Delete?")).toBeDefined();
    expect(screen.getByText("Are you sure")).toBeDefined();
    expect(screen.getByRole("button", { name: "Confirm" })).toBeDefined();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeDefined();
  });

  it("calls onConfirm when the confirm button is clicked", async () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    const user = userEvent.setup();
    render(
      <ConfirmDialog
        title="t"
        message="m"
        onConfirm={onConfirm}
        onCancel={onCancel}
        confirmLabel="Delete"
      />,
    );
    await user.click(screen.getByRole("button", { name: "Delete" }));
    expect(onConfirm).toHaveBeenCalled();
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("calls onCancel when the cancel button is clicked", async () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    const user = userEvent.setup();
    render(
      <ConfirmDialog
        title="t"
        message="m"
        onConfirm={onConfirm}
        onCancel={onCancel}
      />,
    );
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onCancel).toHaveBeenCalled();
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("Escape cancels, Enter confirms", async () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    const user = userEvent.setup();
    render(
      <ConfirmDialog
        title="t"
        message="m"
        onConfirm={onConfirm}
        onCancel={onCancel}
      />,
    );
    await user.keyboard("{Escape}");
    expect(onCancel).toHaveBeenCalled();

    onCancel.mockClear();
    await user.keyboard("{Enter}");
    expect(onConfirm).toHaveBeenCalled();
  });

  it("requireText gates the confirm button behind a typed phrase", async () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    const user = userEvent.setup();
    render(
      <ConfirmDialog
        title="Reindex?"
        message="This wipes transcripts."
        confirmLabel="Reindex"
        requireText="refresh"
        destructive
        onConfirm={onConfirm}
        onCancel={onCancel}
      />,
    );

    const confirmBtn = screen.getByRole("button", { name: "Reindex" });
    expect(confirmBtn).toHaveProperty("disabled", true);

    // Clicking while disabled does nothing.
    await user.click(confirmBtn);
    expect(onConfirm).not.toHaveBeenCalled();

    // Partial match stays disabled.
    const input = screen.getByLabelText(/type refresh to confirm/i);
    await user.type(input, "refres");
    expect(confirmBtn).toHaveProperty("disabled", true);

    // Exact match unlocks.
    await user.type(input, "h");
    expect(confirmBtn).toHaveProperty("disabled", false);
    await user.click(confirmBtn);
    expect(onConfirm).toHaveBeenCalled();
  });

  it("requireText blocks Enter-to-confirm until the phrase matches", async () => {
    const onConfirm = vi.fn();
    const user = userEvent.setup();
    render(
      <ConfirmDialog
        title="Reindex?"
        message="."
        requireText="refresh"
        onConfirm={onConfirm}
        onCancel={vi.fn()}
      />,
    );
    await user.keyboard("{Enter}");
    expect(onConfirm).not.toHaveBeenCalled();

    const input = screen.getByLabelText(/type refresh to confirm/i);
    await user.type(input, "refresh");
    await user.keyboard("{Enter}");
    expect(onConfirm).toHaveBeenCalled();
  });

  it("applies destructive styling when destructive=true", () => {
    render(
      <ConfirmDialog
        title="t"
        message="m"
        destructive
        confirmLabel="Delete"
        onConfirm={noop}
        onCancel={noop}
      />,
    );
    const btn = screen.getByRole("button", { name: "Delete" });
    expect(btn.className).toContain("destructive");
  });
});
