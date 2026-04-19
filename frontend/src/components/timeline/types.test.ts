import { describe, expect, it } from "vitest";

import { makeEvent, textBlock, toolResultBlock, toolUseBlock } from "./test-helpers";
import { isToolResultUser, textPreview, toolUsesIn } from "./types";

describe("timeline/types helpers", () => {
  it("textPreview returns block text for assistant events", () => {
    const e = makeEvent("assistant", {
      blocks: [textBlock(0, "ok I'll read the file")],
    });
    expect(textPreview(e)).toBe("ok I'll read the file");
  });

  it("textPreview falls back to bracketed kind when there is no block text", () => {
    const e = makeEvent("summary");
    expect(textPreview(e)).toBe("[summary]");
  });

  it("isToolResultUser detects tool-result wrappers", () => {
    const wrap = makeEvent("user", {
      blocks: [toolResultBlock(0, "x", "ok")],
    });
    const real = makeEvent("user", {
      blocks: [textBlock(0, "hi")],
    });
    expect(isToolResultUser(wrap)).toBe(true);
    expect(isToolResultUser(real)).toBe(false);
  });

  it("toolUsesIn extracts tool_use blocks", () => {
    const e = makeEvent("assistant", {
      blocks: [
        textBlock(0, "ok"),
        toolUseBlock(1, "t1", "read", { path: "/tmp/a" }, "Read"),
        toolUseBlock(2, "t2", "bash", { command: "ls" }, "Bash"),
      ],
    });
    const tools = toolUsesIn(e);
    expect(tools).toHaveLength(2);
    expect(tools[0]!.name).toBe("read");
    expect(tools[0]!.rawName).toBe("Read");
    expect(tools[1]!.name).toBe("bash");
    expect(tools[1]!.rawName).toBe("Bash");
  });

  it("textPreview includes canonical tool names", () => {
    const e = makeEvent("assistant", {
      blocks: [
        textBlock(0, "canonical path wins"),
        toolUseBlock(1, "t1", "read", { path: "/a" }, "Read"),
      ],
    });
    expect(textPreview(e)).toBe("canonical path wins [tool_use: read]");
  });

  it("long preview is truncated with ellipsis", () => {
    const long = "x".repeat(500);
    const e = makeEvent("user", {
      blocks: [textBlock(0, long)],
    });
    expect(textPreview(e, 50).length).toBeLessThanOrEqual(51);
    expect(textPreview(e, 50).endsWith("…")).toBe(true);
  });
});
