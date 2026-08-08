import { describe, expect, it } from "vitest";

import {
  createClipboardImageUpload,
  imageFromClipboard,
  sanitizePaste,
} from "./clipboard";

describe("sanitizePaste", () => {
  it("strips zero-width and BOM characters", () => {
    const dirty = "hel\u200Blo\u200Cworld\uFEFF!";
    expect(sanitizePaste(dirty)).toBe("helloworld!");
  });

  it("normalizes CRLF to LF", () => {
    expect(sanitizePaste("line1\r\nline2\r\nline3")).toBe(
      "line1\nline2\nline3",
    );
  });

  it("normalizes bare CR to LF", () => {
    expect(sanitizePaste("line1\rline2")).toBe("line1\nline2");
  });

  it("preserves normal text", () => {
    expect(sanitizePaste("git status\n")).toBe("git status\n");
  });

  it("strips word-joiner (U+2060) and Mongolian vowel separator (U+180E)", () => {
    expect(sanitizePaste("ab\u2060cd\u180Eef")).toBe("abcdef");
  });
});

describe("clipboard images", () => {
  it("finds an image item when the clipboard also carries text", () => {
    const image = new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], "image.png", {
      type: "image/png",
    });
    const data = {
      items: [
        { kind: "string", type: "text/plain", getAsFile: () => null },
        { kind: "file", type: "image/png", getAsFile: () => image },
      ],
      files: [],
    } as unknown as DataTransfer;

    expect(imageFromClipboard(data)).toEqual({ file: image, mediaType: "image/png" });
  });

  it("renames a JPEG with the paste timestamp and a conventional extension", () => {
    const image = new File(["jpeg bytes"], "clipboard-image", {
      type: "image/jpeg",
    });
    const upload = createClipboardImageUpload(
      { file: image, mediaType: "image/jpeg" },
      new Date("2026-08-08T12:34:56.789Z"),
    );

    expect(upload.name).toBe("paste-2026-08-08_12-34-56-789Z.jpg");
    expect(upload.type).toBe("image/jpeg");
    expect(upload.size).toBe(image.size);
  });
});
