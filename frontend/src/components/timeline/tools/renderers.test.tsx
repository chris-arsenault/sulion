import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";

import { ToolCallRenderer } from "./renderers";

describe("ToolCallRenderer", () => {
  it("renders an Edit with old/new diff blocks", () => {
    render(
      <ToolCallRenderer
        tool={{
          name: "edit",
          input: {
            file_path: "/tmp/foo.ts",
            old_string: "hello",
            new_string: "hello world",
          },
        }}
      />,
    );
    expect(screen.getByText("/tmp/foo.ts")).toBeDefined();
    expect(screen.getByText("hello")).toBeDefined();
    expect(screen.getByText("hello world")).toBeDefined();
  });

  it("renders a Bash command with description", () => {
    render(
      <ToolCallRenderer
        tool={{
          name: "bash",
          input: { command: "ls -la", description: "list files" },
        }}
      />,
    );
    expect(screen.getByText("list files")).toBeDefined();
    expect(screen.getByText(/ls -la/)).toBeDefined();
  });

  it("renders a Read with path + optional offset/limit", () => {
    render(
      <ToolCallRenderer
        tool={{
          name: "read",
          input: { file_path: "/a/b.txt", offset: 10, limit: 50 },
        }}
      />,
    );
    expect(screen.getByText("/a/b.txt")).toBeDefined();
    expect(
      screen.getByText((t) => /line 10/.test(t) && /limit 50/.test(t)),
    ).toBeDefined();
  });

  it("renders a Grep with pattern and path", () => {
    render(
      <ToolCallRenderer
        tool={{
          name: "grep",
          input: { pattern: "TODO", path: "src/" },
        }}
      />,
    );
    expect(screen.getByText("TODO")).toBeDefined();
    expect(screen.getByText("src/")).toBeDefined();
  });

  it("renders a Task with agent + prompt", () => {
    render(
      <ToolCallRenderer
        tool={{
          name: "task",
          input: {
            subagent_type: "Explore",
            description: "find stuff",
            prompt: "go look around",
          },
        }}
      />,
    );
    expect(screen.getByText(/Explore/)).toBeDefined();
    expect(screen.getByText("find stuff")).toBeDefined();
    expect(screen.getByText(/go look around/)).toBeDefined();
  });

  it("falls back to generic JSON for unknown tools", () => {
    render(
      <ToolCallRenderer
        tool={{
          name: "AsYetUninventedTool",
          input: { x: 1, y: [2, 3] },
        }}
      />,
    );
    // The generic renderer pretty-prints the JSON.
    expect(screen.getByText(/"x": 1/)).toBeDefined();
  });

  it("renders MultiEdit with multiple diff blocks", () => {
    render(
      <ToolCallRenderer
        tool={{
          name: "multi_edit",
          input: {
            file_path: "/tmp/x.ts",
            edits: [
              { old_string: "a", new_string: "b" },
              { old_string: "c", new_string: "d" },
              { old_string: "e", new_string: "f" },
            ],
          },
        }}
      />,
    );
    expect(screen.getByText("/tmp/x.ts")).toBeDefined();
    expect(screen.getByText("3 edits")).toBeDefined();
  });
});
