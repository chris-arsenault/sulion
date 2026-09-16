import { describe, expect, it } from "vitest";

import { classifyMarkdownLink } from "./markdownLinks";

describe("classifyMarkdownLink", () => {
  it("passes http, https and mailto through as external", () => {
    expect(classifyMarkdownLink("https://example.com/a?b=1", "sulion")).toEqual({
      kind: "external",
      href: "https://example.com/a?b=1",
    });
    expect(classifyMarkdownLink("mailto:a@b.c", null)).toEqual({
      kind: "external",
      href: "mailto:a@b.c",
    });
  });

  it("treats a relative path as a repo file", () => {
    expect(
      classifyMarkdownLink("worlds/glass-frontier/research/LITHREN-FIRST-BATCH.md", "tsonu"),
    ).toEqual({ kind: "file", path: "worlds/glass-frontier/research/LITHREN-FIRST-BATCH.md" });
    expect(classifyMarkdownLink("./docs/plan.md", "sulion")).toEqual({
      kind: "file",
      path: "docs/plan.md",
    });
  });

  it("parses a trailing line reference in either common form", () => {
    expect(classifyMarkdownLink("src/a.ts:42", "sulion")).toEqual({
      kind: "file",
      path: "src/a.ts",
      line: 42,
    });
    expect(classifyMarkdownLink("src/a.ts:42:7", "sulion")).toEqual({
      kind: "file",
      path: "src/a.ts",
      line: 42,
    });
    expect(classifyMarkdownLink("src/a.ts#L12-L20", "sulion")).toEqual({
      kind: "file",
      path: "src/a.ts",
      line: 12,
    });
  });

  it("trims an absolute checkout path down to the repo", () => {
    expect(
      classifyMarkdownLink("/home/sulion/repos/tsonu-canon/worlds/x.md", "tsonu-canon"),
    ).toEqual({ kind: "file", path: "worlds/x.md" });
    expect(classifyMarkdownLink("file:///home/u/repos/sulion/docs/a.md", "sulion")).toEqual({
      kind: "file",
      path: "docs/a.md",
    });
  });

  it("is inert without a repo, for fragments, traversal and unsafe schemes", () => {
    expect(classifyMarkdownLink("docs/plan.md", null)).toEqual({ kind: "inert" });
    expect(classifyMarkdownLink("#section", "sulion")).toEqual({ kind: "inert" });
    expect(classifyMarkdownLink("../outside.md", "sulion")).toEqual({ kind: "inert" });
    expect(classifyMarkdownLink("javascript:alert(1)", "sulion")).toEqual({ kind: "inert" });
    expect(classifyMarkdownLink(undefined, "sulion")).toEqual({ kind: "inert" });
  });
});
