import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { ToolCallRenderer } from "./renderers";
import { appCommands } from "../../../state/AppCommands";
import { resetRepoStore, useRepoStore } from "../../../state/RepoStore";
import { ContextMenuHost } from "../../common/ContextMenu";

const LIB_RS_PATH = "src/lib.rs";

const EDIT_TOOL = {
  name: "edit",
  input: {
    file_edits: [
      {
        path: "/tmp/foo.ts",
        operation: "update",
        in_out: { old_text: "hello", new_text: "hello world" },
      },
    ],
  },
};
const BASH_TOOL = {
  name: "bash",
  input: { command: "ls -la", description: "list files" },
};
const READ_TOOL = {
  name: "read",
  input: { path: "/a/b.txt", offset: 10, limit: 50 },
};
const GREP_TOOL = {
  name: "grep",
  input: { pattern: "TODO", path: "src/" },
};
const TASK_TOOL = {
  name: "task",
  input: {
    agent: "Explore",
    description: "find stuff",
    prompt: "go look around",
  },
};
const UNKNOWN_TOOL = {
  name: "AsYetUninventedTool",
  input: { x: 1, y: [2, 3] },
};
const MULTI_EDIT_TOOL = {
  name: "multi_edit",
  input: {
    file_edits: [
      {
        path: "/tmp/x.ts",
        operation: "update",
        in_out: { old_text: "a", new_text: "b" },
      },
      {
        path: "/tmp/x.ts",
        operation: "update",
        in_out: { old_text: "c", new_text: "d" },
      },
      {
        path: "/tmp/x.ts",
        operation: "update",
        in_out: { old_text: "e", new_text: "f" },
      },
    ],
  },
};

const APPLY_PATCH_TOOL = {
  name: "apply_patch",
  operationType: "apply_patch",
  input: {
    file_edits: [
      {
        path: "/repo/a.rs",
        operation: "update",
        diff: "@@\n-before\n+after\n context line",
      },
      {
        path: "/repo/gone.rs",
        operation: "delete",
        diff: "",
      },
    ],
  },
};

const READ_WITH_TOUCHES = {
  name: "read",
  input: { path: LIB_RS_PATH },
  fileTouches: [
    {
      repo: "alpha",
      path: LIB_RS_PATH,
      touch_kind: "inspect",
      is_write: false,
    },
  ],
};

const LARGE_EDIT_TOOL = {
  name: "edit",
  input: {
    file_edits: [
      {
        path: "/tmp/compute.ts",
        operation: "update",
        in_out: {
          old_text: [
            "function compute() {",
            "  const alpha = 1;",
            "  const beta = 2;",
            "  const gamma = 3;",
            "  const delta = 4;",
            "  return alpha + beta + gamma + delta;",
            "  const epsilon = 5;",
            "  const zeta = 6;",
            "  const eta = 7;",
            "  return epsilon + zeta + eta;",
            "}",
            "",
          ].join("\n"),
          new_text: [
            "function compute() {",
            "  const alpha = 1;",
            "  const beta = 2;",
            "  const gamma = 3;",
            "  const delta = 4;",
            "  return alpha + beta + delta + epsilon;",
            "  const epsilon = 5;",
            "  const zeta = 6;",
            "  const eta = 7;",
            "  return epsilon + zeta + eta;",
            "}",
            "",
          ].join("\n"),
        },
      },
    ],
  },
};

const WHITESPACE_ONLY_EDIT_TOOL = {
  name: "edit",
  input: {
    file_edits: [
      {
        path: "/tmp/spacing.ts",
        operation: "update",
        in_out: { old_text: "const value=1;\n", new_text: "const value = 1;\n" },
      },
    ],
  },
};

describe("ToolCallRenderer", () => {
  beforeEach(() => {
    resetRepoStore();
  });

  it("renders an Edit with inline diff content", () => {
    const { container } = render(<ToolCallRenderer tool={EDIT_TOOL} />);
    expect(screen.getByText("/tmp/foo.ts")).toBeDefined();
    expect(screen.getAllByText("hello")).toHaveLength(2);
    const addedParts = Array.from(container.querySelectorAll(".tr-idiff__part--added")).map(
      (node) => node.textContent,
    );
    expect(addedParts).toContain(" world");
  });

  it("renders a Bash command with description", () => {
    render(<ToolCallRenderer tool={BASH_TOOL} />);
    expect(screen.getByText("list files")).toBeDefined();
    expect(screen.getByText(/\$ ls -la/)).toBeDefined();
  });

  it("collapses unchanged edit context in the timeline renderer", () => {
    const { container } = render(<ToolCallRenderer tool={LARGE_EDIT_TOOL} />);

    expect(screen.getByText("/tmp/compute.ts")).toBeDefined();
    expect(screen.getAllByText(/unchanged line/)).toHaveLength(2);
    const removedParts = Array.from(container.querySelectorAll(".tr-idiff__part--removed")).map(
      (node) => node.textContent,
    );
    const addedParts = Array.from(container.querySelectorAll(".tr-idiff__part--added")).map(
      (node) => node.textContent,
    );
    expect(removedParts).toContain("gamma");
    expect(addedParts).toContain("epsilon");
  });

  it("renders a whitespace-only edit placeholder", () => {
    render(<ToolCallRenderer tool={WHITESPACE_ONLY_EDIT_TOOL} />);

    expect(screen.getByText("/tmp/spacing.ts")).toBeDefined();
    expect(screen.getByText("whitespace-only changes omitted")).toBeDefined();
  });

  it("renders a Read with path + optional offset/limit", () => {
    render(<ToolCallRenderer tool={READ_TOOL} />);
    expect(screen.getByText("/a/b.txt")).toBeDefined();
    expect(
      screen.getByText((t) => /line 10/.test(t) && /limit 50/.test(t)),
    ).toBeDefined();
  });

  it("renders a Grep with pattern and path", () => {
    render(<ToolCallRenderer tool={GREP_TOOL} />);
    expect(screen.getByText("TODO")).toBeDefined();
    expect(screen.getByText("src/")).toBeDefined();
  });

  it("renders a Task with agent + prompt", () => {
    render(<ToolCallRenderer tool={TASK_TOOL} />);
    expect(screen.getByText(/Explore/)).toBeDefined();
    expect(screen.getByText("find stuff")).toBeDefined();
    expect(screen.getByText(/go look around/)).toBeDefined();
  });

  it("falls back to generic JSON for unknown tools", () => {
    render(<ToolCallRenderer tool={UNKNOWN_TOOL} />);
    expect(screen.getByText(/"x": 1/)).toBeDefined();
  });

  it("renders MultiEdit with multiple diff blocks", () => {
    render(<ToolCallRenderer tool={MULTI_EDIT_TOOL} />);
    expect(screen.getByText("/tmp/x.ts")).toBeDefined();
    expect(screen.getByText("3 edits")).toBeDefined();
  });

  it("renders apply_patch diff-form entries through the shared renderer", () => {
    const { container } = render(<ToolCallRenderer tool={APPLY_PATCH_TOOL} />);
    // File-edit headers show the operation chip + path.
    expect(screen.getByText("/repo/a.rs")).toBeDefined();
    expect(screen.getByText("/repo/gone.rs")).toBeDefined();
    expect(screen.getAllByText("update")).toHaveLength(1);
    expect(screen.getAllByText("delete")).toHaveLength(1);
    // diff-form lines render via UnifiedDiff with +/- classes.
    const addedLines = Array.from(container.querySelectorAll(".tr-udiff__line--added"));
    const removedLines = Array.from(container.querySelectorAll(".tr-udiff__line--removed"));
    expect(addedLines.some((el) => el.textContent?.includes("+after"))).toBe(true);
    expect(removedLines.some((el) => el.textContent?.includes("-before"))).toBe(true);
    // delete entry has no diff body, so no code block.
    expect(
      container.querySelectorAll('.tr-fe:nth-of-type(2) .tr-udiff').length,
    ).toBe(0);
  });

  it("renders explicit file-touch actions from app data", async () => {
    useRepoStore.setState({
      repos: {
        alpha: {
          git: {
            branch: "main",
            uncommitted_count: 1,
            untracked_count: 0,
            last_commit: null,
            recent_commits: [],
            dirty_by_path: { [LIB_RS_PATH]: " M" },
            diff_stats_by_path: { [LIB_RS_PATH]: { additions: 12, deletions: 3 } },
          },
          dirtyLoadedRevision: 1,
          gitError: null,
          tree: {},
          treeEpoch: 0,
          expanded: new Set(),
          collapsed: new Set(),
          showAll: false,
        },
      },
      loadDirty: vi.fn(),
      toggleDir: vi.fn(),
      expandPath: vi.fn(),
      refresh: vi.fn(),
      setShowAll: vi.fn(),
      loadDir: vi.fn(),
      refreshVisibleDirs: vi.fn(),
    });
    const revealSpy = vi.spyOn(appCommands, "revealFile");
    const openSpy = vi.spyOn(appCommands, "openFile");
    const diffSpy = vi.spyOn(appCommands, "openDiff");

    render(
      <>
        <ToolCallRenderer tool={READ_WITH_TOUCHES} />
        <ContextMenuHost />
      </>,
    );

    expect(screen.getByText("alpha:src/lib.rs")).toBeDefined();
    expect(screen.getByText("+12 -3")).toBeDefined();

    const user = userEvent.setup();
    await user.pointer({
      keys: "[MouseRight]",
      target: screen.getByText("alpha:src/lib.rs"),
    });
    await user.click(screen.getByRole("menuitem", { name: /reveal in file tree/i }));
    await user.pointer({
      keys: "[MouseRight]",
      target: screen.getByText("alpha:src/lib.rs"),
    });
    await user.click(screen.getByRole("menuitem", { name: /open file/i }));
    await user.pointer({
      keys: "[MouseRight]",
      target: screen.getByText("alpha:src/lib.rs"),
    });
    await user.click(screen.getByRole("menuitem", { name: /open diff/i }));

    expect(revealSpy).toHaveBeenCalledWith({ repo: "alpha", path: LIB_RS_PATH });
    expect(openSpy).toHaveBeenCalledWith({ repo: "alpha", path: LIB_RS_PATH });
    expect(diffSpy).toHaveBeenCalledWith({ repo: "alpha", path: LIB_RS_PATH });
  });
});
