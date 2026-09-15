import { describe, expect, it } from "vitest";

import { effectiveDisplayMode, promptInjectionTarget } from "./displayPolicy";

describe("effectiveDisplayMode", () => {
  it("forces timeline mode on mobile without changing desktop choices", () => {
    expect(effectiveDisplayMode("split", true)).toBe("timeline");
    expect(effectiveDisplayMode("terminal", true)).toBe("timeline");
    expect(effectiveDisplayMode("split", false)).toBe("split");
    expect(effectiveDisplayMode("terminal", false)).toBe("terminal");
  });
});

describe("promptInjectionTarget", () => {
  it("sends injected prompts to the timeline box only when the terminal is hidden", () => {
    expect(promptInjectionTarget("timeline")).toBe("timeline");
    expect(promptInjectionTarget("split")).toBe("terminal");
    expect(promptInjectionTarget("terminal")).toBe("terminal");
    expect(promptInjectionTarget(effectiveDisplayMode("split", true))).toBe("timeline");
  });
});
