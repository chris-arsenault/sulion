import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { subscribeToAppCommands } from "../../state/AppCommands";
import { Markdown } from "./Markdown";

const fetchBlob = vi.hoisted(() => vi.fn());
vi.mock("../../api/client", async (original) => ({
  ...(await original<typeof import("../../api/client")>()),
  fetchFileBlob: fetchBlob,
}));

const REPO = "vellum-and-vector";
const TARGET = { repo: REPO };
const DIRECTORY = ".artifacts/harness/";
const ROOT = `/home/sulion/repos/${REPO}/`;
const WORKSPACE_TARGET = {
  repo: "atlas",
  workspaceId: "ws-1",
  rootPath: "/home/sulion/workspaces/atlas/ws-1",
};
const FILENAMES = [
  "raised5-plates.png",
  "raised5-heights.png",
  "raised4-heights.png",
];
const createObjectURL = vi.fn(() => "blob:local-preview");
const revokeObjectURL = vi.fn();

describe("Markdown local file references", () => {
  beforeEach(() => {
    fetchBlob
      .mockReset()
      .mockResolvedValue(new Blob(["image"], { type: "image/png" }));
    createObjectURL.mockClear();
    revokeObjectURL.mockClear();
    vi.stubGlobal(
      "URL",
      class extends URL {
        static createObjectURL = createObjectURL;
        static revokeObjectURL = revokeObjectURL;
      },
    );
  });
  afterEach(() => vi.unstubAllGlobals());

  it("resolves the directory-and-filename list from the recent Claude turn and opens images", async () => {
    const files = FILENAMES.map((name) => `- \`${name}\` (draft)`).join("\n");
    const source = `Pictures are in \`${ROOT}${DIRECTORY}\`:\n${files}`;
    const seen: unknown[] = [];
    const unsubscribe = subscribeToAppCommands((command) => seen.push(command));
    render(<Markdown source={source} fileTarget={TARGET} />);
    const images = await screen.findAllByRole("img");
    expect(images).toHaveLength(3);
    for (const name of FILENAMES) {
      const path = DIRECTORY + name;
      expect(fetchBlob).toHaveBeenCalledWith(
        `/api/repos/${REPO}/file/raw?path=${encodeURIComponent(path)}`,
      );
      expect(screen.getByRole("img", { name }).getAttribute("src")).toBe(
        "blob:local-preview",
      );
    }
    fireEvent.click(images[0]!);
    expect(seen).toEqual([
      {
        type: "open-file",
        repo: REPO,
        workspaceId: undefined,
        path: `${DIRECTORY}${FILENAMES[0]}`,
        line: undefined,
      },
    ]);
    unsubscribe();
  });

  it("recognizes full paths and unformatted filename lists without linking surrounding punctuation", async () => {
    const source = `The picture is at \`${ROOT}${DIRECTORY}heights-raised4.png\`.\n\nPictures are in ${ROOT}${DIRECTORY}:\n- raised5-plates.png (plate edges)\n- raised5-heights.png.`;
    const { container } = render(
      <Markdown source={source} fileTarget={TARGET} />,
    );
    expect(await screen.findAllByRole("img")).toHaveLength(3);
    const links = Array.from(container.querySelectorAll("a.md__file-link"));
    expect(links.map((link) => link.getAttribute("href"))).toEqual([
      `${DIRECTORY}heights-raised4.png`,
      `${DIRECTORY}raised5-plates.png`,
      `${DIRECTORY}raised5-heights.png`,
    ]);
    expect(container.textContent).toContain("raised5-heights.png.");
  });

  it("renders Markdown image links, image syntax, and reference images without duplicating or nesting anchors", async () => {
    const source = `[draft](${ROOT}${DIRECTORY}first.png)\n\n![second](${DIRECTORY}second.svg)\n\n[![third](${DIRECTORY}third.png)](${DIRECTORY}third.png)\n\n![fourth][figure]\n\n[figure]: ${DIRECTORY}fourth.png`;
    const { container } = render(
      <Markdown source={source} fileTarget={TARGET} />,
    );
    expect(await screen.findAllByRole("img")).toHaveLength(4);
    expect(fetchBlob).toHaveBeenCalledTimes(4);
    expect(container.querySelector("a a")).toBeNull();
    expect(container.querySelectorAll("a.md__file-link")).toHaveLength(4);
  });

  it("keeps fenced code, commands, link labels and unsafe or out-of-repository paths literal", async () => {
    const source =
      "```text\n/home/sulion/repos/vellum-and-vector/.artifacts/test.png\n```\n\nRun `pnpm test`. See [literal.png](docs/notes.md).\n\n`../outside.png` `/tmp/outside.png` `javascript:alert(1)`";
    const { container } = render(
      <Markdown source={source} fileTarget={TARGET} />,
    );
    expect(container.querySelectorAll("a.md__file-link")).toHaveLength(1);
    expect(container.querySelector("pre code")?.textContent).toContain(
      "test.png",
    );
    expect(fetchBlob).not.toHaveBeenCalled();
  });

  it("resets a filename directory at a new heading and supports source line links", () => {
    const source = `Files in \`${ROOT}docs/\`:\n- \`notes.md\`\n\n## Code\nSee \`src/main.ts:42:7\` and \`README.md\`.`;
    const { container } = render(
      <Markdown source={source} fileTarget={TARGET} />,
    );
    expect(
      Array.from(container.querySelectorAll("a")).map((a) =>
        a.getAttribute("href"),
      ),
    ).toEqual(["docs/notes.md", "src/main.ts#L42", "README.md"]);
  });

  it("resolves absolute workspace references using the workspace raw-file route", async () => {
    render(
      <Markdown
        source={`\`${WORKSPACE_TARGET.rootPath}/plots/draft.png\``}
        fileTarget={WORKSPACE_TARGET}
      />,
    );
    await screen.findByRole("img");
    expect(fetchBlob).toHaveBeenCalledWith(
      "/api/workspaces/ws-1/file/raw?path=plots%2Fdraft.png",
    );
  });

  it("keeps the file link available when the preview fails", async () => {
    fetchBlob.mockRejectedValueOnce(new Error("File not found"));
    const { container } = render(
      <Markdown source="`missing.png`" fileTarget={TARGET} />,
    );
    expect(await screen.findByRole("status")).toHaveProperty(
      "textContent",
      "Image preview unavailable: File not found",
    );
    expect(
      container.querySelector("a.md__file-link")?.getAttribute("href"),
    ).toBe("missing.png");
  });

  it("releases image object URLs on unmount", async () => {
    const { unmount } = render(
      <Markdown source="`draft.png`" fileTarget={TARGET} />,
    );
    await screen.findByRole("img");
    unmount();
    await waitFor(() =>
      expect(revokeObjectURL).toHaveBeenCalledWith("blob:local-preview"),
    );
  });

  it("does not guess local paths without a repository context", () => {
    const { container } = render(
      <Markdown source={`\`${ROOT}${DIRECTORY}draft.png\``} />,
    );
    expect(container.querySelector("a")).toBeNull();
    expect(fetchBlob).not.toHaveBeenCalled();
  });
});
