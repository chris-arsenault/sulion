import { describe, expect, it } from "vitest";

import {
  extractPromptTemplateVariables,
  renderPromptTemplate,
} from "./promptTemplate";

describe("promptTemplate", () => {
  it("extracts variables in first-seen order and ignores escaped dollars", () => {
    expect(
      extractPromptTemplateVariables(
        "do item $n for $repo, then repeat $n. keep $$literal and $5",
      ),
    ).toEqual([
      { name: "n", occurrences: 2 },
      { name: "repo", occurrences: 1 },
    ]);
  });

  it("extracts uppercase variables", () => {
    expect(extractPromptTemplateVariables("Implement item $N.")).toEqual([
      { name: "N", occurrences: 1 },
    ]);
  });

  it("renders variables while preserving literal dollars", () => {
    expect(
      renderPromptTemplate("do item $n for $$5 in $repo", {
        n: "42",
        repo: "sulion",
      }),
    ).toBe("do item 42 for $5 in sulion");
  });

  it("can combine a literal dollar with a following variable", () => {
    expect(renderPromptTemplate("cost $$$amount", { amount: "12" })).toBe(
      "cost $12",
    );
  });
});
