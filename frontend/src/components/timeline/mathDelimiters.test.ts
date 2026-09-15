import { describe, expect, it } from "vitest";

import { normalizeMathDelimiters } from "./mathDelimiters";

describe("normalizeMathDelimiters", () => {
  it("returns text without LaTeX delimiters unchanged", () => {
    const src = "plain *markdown* with $$x$$ and `code`";
    expect(normalizeMathDelimiters(src)).toBe(src);
  });

  it("rewrites a multi-line display block into a $$ flow block", () => {
    const src = [
      "One candidate equation is:",
      "",
      "\\[",
      "\\theta_{es}",
      "=",
      "\\frac{a_{es}\\,\\hat c_s}",
      "{1+\\sum_j a_{ej}\\,\\hat c_j}",
      "\\]",
      "",
      "Here:",
    ].join("\n");
    expect(normalizeMathDelimiters(src)).toBe(
      [
        "One candidate equation is:",
        "",
        "",
        "$$",
        "\\theta_{es}\n=\n\\frac{a_{es}\\,\\hat c_s}\n{1+\\sum_j a_{ej}\\,\\hat c_j}",
        "$$",
        "",
        "",
        "Here:",
      ].join("\n"),
    );
  });

  it("splits display math out of a surrounding paragraph", () => {
    expect(normalizeMathDelimiters("Consider \\[x^2\\] here.")).toBe(
      "Consider \n\n$$\nx^2\n$$\n\n here.",
    );
  });

  it("rewrites inline math into double-dollar text math", () => {
    expect(
      normalizeMathDelimiters(
        "- \\(a_{es}\\) is the affinity between \\(e\\) and \\(s\\);",
      ),
    ).toBe("- $$a_{es}$$ is the affinity between $$e$$ and $$s$$;");
  });

  it("leaves delimiters inside fenced code alone", () => {
    const src = ["```sh", "grep '\\[' file", "```", "", "\\(x\\)"].join("\n");
    expect(normalizeMathDelimiters(src)).toBe(
      ["```sh", "grep '\\[' file", "```", "", "$$x$$"].join("\n"),
    );
  });

  it("leaves delimiters inside inline code alone", () => {
    expect(normalizeMathDelimiters("use `\\(` and ``a \\[b\\] `` then \\(y\\)")).toBe(
      "use `\\(` and ``a \\[b\\] `` then $$y$$",
    );
  });

  it("leaves an unmatched opener untouched", () => {
    expect(normalizeMathDelimiters("stray \\[ bracket")).toBe("stray \\[ bracket");
    expect(normalizeMathDelimiters("stray \\( paren")).toBe("stray \\( paren");
  });

  it("does not treat an escaped backslash as an opener", () => {
    expect(normalizeMathDelimiters("path \\\\[x]")).toBe("path \\\\[x]");
  });
});
