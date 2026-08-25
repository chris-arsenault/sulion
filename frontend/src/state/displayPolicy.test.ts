import { describe, expect, it } from "vitest";

import { effectiveDisplayMode } from "./displayPolicy";

describe("effectiveDisplayMode", () => {
  it("forces timeline mode on mobile without changing desktop choices", () => {
    expect(effectiveDisplayMode("split", true)).toBe("timeline");
    expect(effectiveDisplayMode("terminal", true)).toBe("timeline");
    expect(effectiveDisplayMode("split", false)).toBe("split");
    expect(effectiveDisplayMode("terminal", false)).toBe("terminal");
  });
});
