// Clipboard helpers split out of TerminalPane so sanitize logic is
// independently testable. The terminal component wires these up to
// xterm.js key/paste/context-menu hooks.
//
// Context-sensitivity note: navigator.clipboard (readText/writeText)
// requires a "secure context" — HTTPS or localhost. On a LAN-HTTP
// origin (our TrueNAS deploy at 192.168.66.3:30080) writeText is
// typically blocked, so we fall back to document.execCommand('copy')
// via a transient textarea. readText has no such fallback — paste on
// HTTP happens via the native `paste` event on keystroke gesture,
// not a JS-initiated read.

/** Zero-width and "invisible" chars that ride along in clipboard data
 * from rich sources (web pages, Slack, editors). Removing them
 * prevents the classic "paste looks right but the shell sees junk"
 * bug. */
const INVISIBLE_CHARS = /[\u200B-\u200D\uFEFF\u2060\u180E]/g;

const IMAGE_EXTENSIONS: Record<string, string> = {
  "image/avif": "avif",
  "image/bmp": "bmp",
  "image/gif": "gif",
  "image/heic": "heic",
  "image/heif": "heif",
  "image/jpeg": "jpg",
  "image/png": "png",
  "image/svg+xml": "svg",
  "image/tiff": "tiff",
  "image/vnd.microsoft.icon": "ico",
  "image/webp": "webp",
  "image/x-icon": "ico",
};
const IMAGE_FILENAME = /\.(avif|bmp|gif|heic|heif|ico|jpe?g|png|svg|tiff?|webp)$/i;

export interface ClipboardImage {
  file: File;
  mediaType: string;
}

export function sanitizePaste(text: string): string {
  return text
    .replace(INVISIBLE_CHARS, "")
    .replace(/\r\n/g, "\n")
    .replace(/\r/g, "\n");
}

/** Return the first image file carried by a native paste event. Clipboard
 * implementations vary: screenshots usually appear in `items`, while copied
 * image files may only appear in `files`. */
export function imageFromClipboard(data: DataTransfer): ClipboardImage | null {
  for (const item of Array.from(data.items ?? [])) {
    if (item.kind !== "file") continue;
    const file = item.getAsFile();
    if (!file) continue;
    const mediaType = file.type || item.type;
    if (isImageFile(file, mediaType)) return { file, mediaType };
  }

  for (const file of Array.from(data.files ?? [])) {
    if (isImageFile(file, file.type)) {
      return { file, mediaType: file.type };
    }
  }
  return null;
}

/** Give an anonymous clipboard image a stable, shell-friendly filename while
 * preserving its bytes and media type. */
export function createClipboardImageUpload(
  image: ClipboardImage,
  now = new Date(),
): File {
  const timestamp = now.toISOString().replace(/[:.]/g, "-").replace("T", "_");
  const extension = imageExtension(image);
  return new File([image.file], `paste-${timestamp}.${extension}`, {
    type: image.mediaType || image.file.type,
    lastModified: now.getTime(),
  });
}

export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // Fall through to execCommand.
  }
  return execCommandCopy(text);
}

export async function readClipboard(): Promise<string | null> {
  try {
    if (navigator.clipboard?.readText) {
      return await navigator.clipboard.readText();
    }
  } catch {
    // Secure-context only; the caller should handle null by telling
    // the user to paste via Ctrl+V instead.
  }
  return null;
}

function execCommandCopy(text: string): boolean {
  const ta = document.createElement("textarea");
  ta.value = text;
  ta.setAttribute("readonly", "");
  ta.style.position = "fixed";
  ta.style.left = "-9999px";
  ta.style.top = "0";
  document.body.appendChild(ta);
  ta.select();
  try {
    // `document.execCommand` is formally deprecated but still works on
    // HTTP contexts where navigator.clipboard doesn't — and it does
    // so synchronously inside a user-gesture handler.
    return document.execCommand("copy");
  } finally {
    ta.remove();
  }
}

function isImageFile(file: File, mediaType: string): boolean {
  return mediaType.toLowerCase().startsWith("image/") || IMAGE_FILENAME.test(file.name);
}

function imageExtension(image: ClipboardImage): string {
  const mediaType = image.mediaType.toLowerCase();
  const known = IMAGE_EXTENSIONS[mediaType];
  if (known) return known;

  const filenameMatch = image.file.name.match(IMAGE_FILENAME);
  if (filenameMatch?.[1]) {
    return filenameMatch[1].toLowerCase().replace("jpeg", "jpg");
  }
  return "image";
}
