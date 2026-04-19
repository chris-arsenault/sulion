import { describe, expect, it } from "vitest";

import type { TimelineEvent } from "../../api/types";
import {
  flattenContent,
  isToolResultUser,
  textPreview,
  toolUsesIn,
} from "./types";

function ev(kind: string, payload: unknown): TimelineEvent {
  return {
    byte_offset: 0,
    timestamp: "2025-01-01T00:00:00Z",
    kind,
    payload,
  };
}

describe("timeline/types helpers", () => {
  it("flattens string content as-is", () => {
    expect(flattenContent("hello")).toBe("hello");
  });

  it("flattens array content with text + tool_use + tool_result", () => {
    const s = flattenContent([
      { type: "text", text: "starting" },
      { type: "tool_use", id: "x", name: "Read", input: {} },
      { type: "tool_result", tool_use_id: "x", content: "result text" },
    ]);
    expect(s).toContain("starting");
    expect(s).toContain("[tool_use: Read]");
    expect(s).toContain("[tool_result] result text");
  });

  it("textPreview returns message text for assistant events", () => {
    const e = ev("assistant", {
      type: "assistant",
      message: {
        role: "assistant",
        content: [{ type: "text", text: "ok I'll read the file" }],
      },
    });
    expect(textPreview(e)).toBe("ok I'll read the file");
  });

  it("textPreview falls back to bracketed kind when no message", () => {
    const e = ev("summary", { type: "summary" });
    expect(textPreview(e)).toBe("[summary]");
  });

  it("isToolResultUser detects tool-result wrappers", () => {
    const wrap = ev("user", {
      type: "user",
      message: {
        role: "user",
        content: [{ type: "tool_result", tool_use_id: "x", content: "ok" }],
      },
    });
    const real = ev("user", {
      type: "user",
      message: { role: "user", content: [{ type: "text", text: "hi" }] },
    });
    expect(isToolResultUser(wrap)).toBe(true);
    expect(isToolResultUser(real)).toBe(false);
  });

  it("toolUsesIn extracts tool_use blocks from assistant content", () => {
    const e = ev("assistant", {
      type: "assistant",
      message: {
        content: [
          { type: "text", text: "ok" },
          { type: "tool_use", id: "t1", name: "Read", input: { path: "/tmp/a" } },
          { type: "tool_use", id: "t2", name: "Bash", input: { cmd: "ls" } },
        ],
      },
    });
    const tools = toolUsesIn(e);
    expect(tools).toHaveLength(2);
    // toolUsesIn returns canonical names (the ingester normalises);
    // rawName preserves the agent-specific original for display.
    expect(tools[0]!.name).toBe("read");
    expect(tools[0]!.rawName).toBe("Read");
    expect(tools[1]!.name).toBe("bash");
    expect(tools[1]!.rawName).toBe("Bash");
  });

  it("prefers canonical blocks when the event carries them", () => {
    const e: TimelineEvent = {
      byte_offset: 0,
      timestamp: "2025-01-01T00:00:00Z",
      kind: "assistant",
      payload: {}, // intentionally empty — blocks should win
      blocks: [
        { ord: 0, kind: "text", text: "canonical path wins" },
        {
          ord: 1,
          kind: "tool_use",
          tool_id: "t1",
          tool_name: "Read",
          tool_name_canonical: "read",
          tool_input: { file_path: "/a" },
        },
      ],
    };
    // text extraction comes from blocks, not payload
    const tools = toolUsesIn(e);
    expect(tools).toHaveLength(1);
    expect(tools[0]!.name).toBe("read");
    expect(tools[0]!.rawName).toBe("Read");
    expect(textPreview(e)).toBe("canonical path wins [tool_use: read]");
  });

  it("falls back to legacy payload for events without blocks", () => {
    // Pre-backfill events arrive with blocks empty; helpers must still
    // work by walking the raw payload shape.
    const e: TimelineEvent = {
      byte_offset: 0,
      timestamp: "2025-01-01T00:00:00Z",
      kind: "assistant",
      payload: {
        type: "assistant",
        message: {
          role: "assistant",
          content: [{ type: "text", text: "legacy path" }],
        },
      },
      blocks: [],
    };
    expect(textPreview(e)).toBe("legacy path");
  });

  it("long preview is truncated with ellipsis", () => {
    const long = "x".repeat(500);
    const e = ev("user", {
      type: "user",
      message: { role: "user", content: long },
    });
    expect(textPreview(e, 50).length).toBeLessThanOrEqual(51);
    expect(textPreview(e, 50).endsWith("…")).toBe(true);
  });
});
