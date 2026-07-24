import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { InlineCodeDiff } from "./inlineCodeDiff";
import { buildInlineCodeDiff } from "./inlineCodeDiffModel";

function makeFunction(returnLine: string) {
  return [
    "function compute() {",
    "  const alpha = 1;",
    "  const beta = 2;",
    "  const gamma = 3;",
    "  const delta = 4;",
    returnLine,
    "  const epsilon = 5;",
    "  const zeta = 6;",
    "  const eta = 7;",
    "  return epsilon + zeta + eta;",
    "}",
    "",
  ].join("\n");
}

describe("buildInlineCodeDiff", () => {
  it("marks whitespace-only edits without inventing a content diff", () => {
    const model = buildInlineCodeDiff(
      "const value=1;\nconst label = value;\n",
      "const value = 1;\nconst label=value;\n",
    );

    expect(model.state).toBe("whitespace_only");
    expect(model.rows).toEqual([]);
  });

  it("collapses unchanged regions around a focused code edit", () => {
    const model = buildInlineCodeDiff(
      makeFunction("  return alpha + beta + gamma + delta;"),
      makeFunction("  return alpha + beta + delta + epsilon;"),
    );

    expect(model.state).toBe("changed");
    expect(model.rows.some((row) => row.kind === "collapsed")).toBe(true);

    const removedRow = model.rows.find((row) => row.kind === "removed");
    const addedRow = model.rows.find((row) => row.kind === "added");

    expect(removedRow && "parts" in removedRow).toBeTruthy();
    expect(addedRow && "parts" in addedRow).toBeTruthy();
    expect(
      removedRow?.kind === "removed" &&
        removedRow.parts.some((part) => part.kind === "removed" && part.value.includes("gamma")),
    ).toBe(true);
    expect(
      addedRow?.kind === "added" &&
        addedRow.parts.some((part) => part.kind === "added" && part.value.includes("epsilon")),
    ).toBe(true);
  });
});

describe("InlineCodeDiff", () => {
  it("renders a collapsed inline diff instead of full before/after blobs", () => {
    const { container } = render(
      <InlineCodeDiff
        oldText={makeFunction("  return alpha + beta + gamma + delta;")}
        newText={makeFunction("  return alpha + beta + delta + epsilon;")}
      />,
    );

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

  it("renders a dedicated whitespace-only placeholder", () => {
    render(<InlineCodeDiff oldText={"const value=1;\n"} newText={"const value = 1;\n"} />);

    expect(screen.getByText("whitespace-only changes omitted")).toBeDefined();
  });
});
