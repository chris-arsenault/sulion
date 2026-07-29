import { describe, expect, it } from "vitest";

import { chooseRenderKind } from "./fileRenderKind";
import type { FileResponse } from "../api/types";

function fileResponse(overrides: Partial<FileResponse> = {}): FileResponse {
  return {
    path: "docs/diagram.svg",
    size: 128,
    mime: "image/svg+xml",
    binary: false,
    truncated: false,
    content: "<svg xmlns='http://www.w3.org/2000/svg'></svg>",
    ...overrides,
  };
}

/** Repo files are agent-authored. An inlined <svg> runs its own event handlers
 * even though innerHTML suppresses <script>, so SVG must never reach a render
 * kind that carries markup — it goes through the blob <img> path instead. */
describe("chooseRenderKind: SVG is never inlined as markup", () => {
  it("renders a hostile SVG through the image path, not as markup", () => {
    const hostile = fileResponse({
      content:
        "<svg xmlns='http://www.w3.org/2000/svg'>" +
        "<animate onbegin='globalThis.__pwned = true' attributeName='x' dur='1s'/>" +
        "</svg>",
    });

    const kind = chooseRenderKind(hostile, false);

    expect(kind.kind).toBe("image-binary");
    // No variant may carry the source through to the DOM as HTML.
    expect(JSON.stringify(kind)).not.toContain("onbegin");
  });

  it("uses the image path for a benign SVG too", () => {
    expect(chooseRenderKind(fileResponse(), false).kind).toBe("image-binary");
  });

  it("shows SVG source as inert text when the raw toggle is on", () => {
    const kind = chooseRenderKind(fileResponse(), true);

    expect(kind.kind).toBe("raw");
    // "raw" is rendered inside <pre> as a text child, so markup stays inert.
    expect(kind).toMatchObject({ code: expect.stringContaining("<svg") });
  });

  it("still routes genuine binary images through the image path", () => {
    const png = fileResponse({
      path: "docs/shot.png",
      mime: "image/png",
      binary: true,
      content: null,
    });

    expect(chooseRenderKind(png, false).kind).toBe("image-binary");
  });
});
