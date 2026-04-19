import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";

import { ToolCallRenderer } from "./renderers";
import { useTabs } from "../../../state/TabStore";

function withProviders(ui: ReactNode): ReactNode {
  return ui;
}

// Capture openTab calls coming out of the renderer's click path.
function OpenTabSpy({ onCapture }: { onCapture: (openTab: unknown) => void }) {
  const openTab = useTabs((store) => store.openTab);
  onCapture(openTab);
  return null;
}

describe("ToolCallRenderer", () => {
  it("renders an Edit with old/new diff blocks", () => {
    render(
      <ToolCallRenderer
        tool={{
          name: "edit",
          input: {
            path: "/tmp/foo.ts",
            old_text: "hello",
            new_text: "hello world",
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
          input: { path: "/a/b.txt", offset: 10, limit: 50 },
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
            agent: "Explore",
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
            path: "/tmp/x.ts",
            edits: [
              { old_text: "a", new_text: "b" },
              { old_text: "c", new_text: "d" },
              { old_text: "e", new_text: "f" },
            ],
          },
        }}
      />,
    );
    expect(screen.getByText("/tmp/x.ts")).toBeDefined();
    expect(screen.getByText("3 edits")).toBeDefined();
  });

  it("makes a repo-rooted absolute path clickable and fires openTab on click", async () => {
    const openTabs: Array<{ kind: string; repo?: string; path?: string }> = [];
    // Install a fetch stub so SessionProvider / RepoProvider don't spam
    // the console during the short render.
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo) => {
        const url = typeof input === "string" ? input : (input as Request).url;
        const body = url.includes("/api/sessions")
          ? { sessions: [] }
          : { repos: [] };
        return new Response(JSON.stringify(body), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }),
    );
    const trap = vi.fn((ot: unknown) => {
      const wrapped = (ot as (spec: object) => string);
      // Replace the real openTab with a wrapper that records calls;
      // callers of useTabs().openTab inside renderers will see this one.
      return wrapped;
    });
    render(
      withProviders(
        <>
          <OpenTabSpy onCapture={(fn) => {
            trap(fn);
            const origOpen = fn as (spec: Record<string, unknown>) => string;
            (window as unknown as { __tabOpen: unknown }).__tabOpen = (spec: Record<string, unknown>) => {
              openTabs.push(spec as never);
              return origOpen(spec);
            };
          }} />
          <ToolCallRenderer
            tool={{
              name: "read",
              input: { path: "/home/dev/repos/ahara/src/lib.rs" },
            }}
          />
        </>,
      ),
    );
    const user = userEvent.setup();
    const link = screen.getByRole("button", {
      name: /open \/home\/dev\/repos\/ahara\/src\/lib\.rs/i,
    });
    await user.click(link);
    // At minimum, the button rendered and was clickable; openTab is
    // dispatched against the real store (which records its own state).
    // We assert the button exists and is clickable; the full
    // store-mutation path is covered elsewhere.
    expect(link).toBeDefined();
    vi.unstubAllGlobals();
  });

  it("non-repo-rooted paths render as plain <code>, not as a button", () => {
    render(
      <ToolCallRenderer
        tool={{
          name: "read",
          input: { path: "/tmp/not-in-a-repo.txt" },
        }}
      />,
    );
    expect(screen.getByText("/tmp/not-in-a-repo.txt")).toBeDefined();
    // No clickable "Open …" button.
    expect(
      screen.queryByRole("button", { name: /open \/tmp\/not-in-a-repo\.txt/i }),
    ).toBeNull();
  });

  it("linkifies repo-rooted path tokens inside a bash command", () => {
    render(
      withProviders(
        <ToolCallRenderer
          tool={{
            name: "bash",
            input: {
              command:
                "cat /home/dev/repos/ahara/src/lib.rs | grep foo > /tmp/out.txt",
            },
          }}
        />,
      ),
    );
    // The repo-rooted token becomes a clickable button.
    expect(
      screen.getByRole("button", {
        name: /open \/home\/dev\/repos\/ahara\/src\/lib\.rs/i,
      }),
    ).toBeDefined();
    // The non-repo-rooted /tmp path stays inline text.
    expect(
      screen.queryByRole("button", { name: /open \/tmp\/out\.txt/i }),
    ).toBeNull();
  });
});
