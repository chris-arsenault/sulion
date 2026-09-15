import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Markdown } from "./Markdown";

describe("Markdown math", () => {
  it("renders LaTeX display math as a KaTeX display block", () => {
    const source = [
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
    const { container } = render(<Markdown source={source} />);
    const display = container.querySelector(".katex-display");
    expect(display).not.toBeNull();
    expect(display?.querySelector(".mfrac")).not.toBeNull();
    expect(container.querySelector("annotation")?.textContent).toContain(
      "\\theta_{es}",
    );
    // The paragraphs around it survive as prose.
    expect(container.textContent).toContain("One candidate equation is:");
    expect(container.textContent).toContain("Here:");
  });

  it("renders LaTeX inline math inline within its paragraph", () => {
    const { container } = render(
      <Markdown source={"where \\(a_{es}\\) is the affinity"} />,
    );
    const p = container.querySelector("p");
    expect(p?.querySelector(".katex")).not.toBeNull();
    expect(p?.querySelector(".katex-display")).toBeNull();
    expect(p?.textContent).toMatch(/^where .*is the affinity$/);
  });

  it("renders a $$ block from Claude-style markdown", () => {
    const { container } = render(
      <Markdown source={"$$\nE = mc^2\n$$"} />,
    );
    expect(container.querySelector(".katex-display")).not.toBeNull();
  });

  it("does not pair single dollars in prose into math", () => {
    const { container } = render(
      <Markdown source={"set $HOME and $PATH before running"} />,
    );
    expect(container.querySelector(".katex")).toBeNull();
    expect(container.textContent).toBe("set $HOME and $PATH before running");
  });

  it("keeps delimiters inside code literal", () => {
    const { container } = render(
      <Markdown source={"run `grep '\\[' f` first\n\n```\n\\(x\\)\n```"} />,
    );
    expect(container.querySelector(".katex")).toBeNull();
    expect(container.querySelector("code")?.textContent).toBe("grep '\\[' f");
    expect(container.querySelector("pre code")?.textContent).toBe("\\(x\\)\n");
  });

  it("renders malformed TeX as source instead of throwing", () => {
    const { container } = render(
      <Markdown source={"\\(\\frac{a\\)"} />,
    );
    expect(container.querySelector(".katex-error")).not.toBeNull();
    expect(container.textContent).toContain("\\frac{a");
  });
});
