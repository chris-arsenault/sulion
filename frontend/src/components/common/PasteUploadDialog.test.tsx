import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { uploadFile } from "../../api/client";
import { PasteUploadDialog, type PendingAttachment } from "./PasteUploadDialog";

vi.mock("../../api/client", () => ({ uploadFile: vi.fn() }));
const pending: PendingAttachment = { kind: "text", raw: "large text", size: 10, lines: 1,
  repo: "repo", sessionId: "session", workspaceId: "workspace" };
beforeEach(() => vi.clearAllMocks());

describe("clipboard upload", () => {
  it("retains failed content, retries the same File and never silently inserts raw text", async () => {
    vi.mocked(uploadFile).mockRejectedValueOnce(new Error("storage unavailable"))
      .mockResolvedValueOnce({ path: "/workspace/paste.txt", size: 10 });
    const insert = vi.fn();
    const close = vi.fn();
    const user = userEvent.setup();
    render(<PasteUploadDialog pending={pending} onInsert={insert} onClose={close} />);
    await user.click(screen.getByRole("button", { name: "Save as file" }));
    await screen.findByText(/storage unavailable/);
    expect(insert).not.toHaveBeenCalled();
    expect(close).not.toHaveBeenCalled();
    const file = vi.mocked(uploadFile).mock.calls[0][2];
    await user.click(screen.getByRole("button", { name: "Retry upload" }));
    await waitFor(() => expect(insert).toHaveBeenCalledWith("/workspace/paste.txt "));
    expect(vi.mocked(uploadFile).mock.calls[1][2]).toBe(file);
    expect(uploadFile).toHaveBeenCalledWith(pending, ".sulion-paste", file);
  });

  it("does not insert after the originating consumer is unmounted", async () => {
    let finish!: (value: {path: string; size: number}) => void;
    vi.mocked(uploadFile).mockReturnValue(new Promise((resolve) => { finish = resolve; }));
    const insert = vi.fn();
    const view = render(<PasteUploadDialog pending={pending} onInsert={insert} onClose={vi.fn()} />);
    await userEvent.click(screen.getByRole("button", { name: "Save as file" }));
    view.unmount();
    finish({ path: "/workspace/paste.txt", size: 10 });
    await Promise.resolve();
    expect(insert).not.toHaveBeenCalled();
  });

  it("requires an explicit Paste inline choice and Cancel discards without inserting", async () => {
    const insert = vi.fn();
    const close = vi.fn();
    render(<PasteUploadDialog pending={pending} onInsert={insert} onClose={close} />);
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(insert).not.toHaveBeenCalled();
    expect(close).toHaveBeenCalledOnce();
    await userEvent.click(screen.getByRole("button", { name: "Paste inline" }));
    expect(insert).toHaveBeenCalledWith("large text");
  });
});
