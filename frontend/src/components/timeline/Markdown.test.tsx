import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { subscribeToAppCommands } from "../../state/AppCommands";
import { Markdown } from "./Markdown";

const FILE_TARGET = { repo: "tsonu", workspaceId: "ws-1" };

describe("Markdown links", () => {
  it("opens a repo-relative link as a file tab without navigating", () => {
    const seen: unknown[] = [];
    const unsubscribe = subscribeToAppCommands((command) => seen.push(command));
    const { container } = render(
      <Markdown
        source={"Start with the [review brief](worlds/research/BRIEF.md:12)."}
        fileTarget={FILE_TARGET}
      />,
    );
    const anchor = container.querySelector("a.md__file-link") as HTMLAnchorElement;
    expect(anchor.textContent).toBe("review brief");
    expect(anchor.title).toBe("Open worlds/research/BRIEF.md:12");

    const click = fireEvent.click(anchor);
    expect(click).toBe(false); // default prevented: no browser navigation
    expect(seen).toEqual([
      {
        type: "open-file",
        repo: "tsonu",
        workspaceId: "ws-1",
        path: "worlds/research/BRIEF.md",
        line: 12,
      },
    ]);
    unsubscribe();
  });

  it("opens absolute URLs in a new browser tab", () => {
    const { container } = render(
      <Markdown source={"see https://example.com/x and [y](http://y.test)"} />,
    );
    const anchors = Array.from(container.querySelectorAll("a"));
    expect(anchors).toHaveLength(2);
    for (const a of anchors) {
      expect(a.getAttribute("target")).toBe("_blank");
      expect(a.getAttribute("rel")).toContain("noopener");
    }
  });

  it("renders a relative link as text when no repo context exists", () => {
    const { container } = render(<Markdown source={"[brief](docs/brief.md)"} />);
    expect(container.querySelector("a")).toBeNull();
    expect(container.querySelector(".md__dead-link")?.textContent).toBe("brief");
  });
});

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
