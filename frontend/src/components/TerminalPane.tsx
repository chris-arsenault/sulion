// Live xterm.js pane. Mounts xterm imperatively in a useEffect keyed on
// sessionId. React is NOT in the rendering path for PTY bytes — the
// WebSocket writes straight into xterm.write; xterm.onData flows directly
// into conn.sendInput. Load-bearing for latency (CLAUDE.md invariant #2).
//
// Copy/paste model (Windows Terminal-ish):
//   - Right-click: if there's a selection, copy it; otherwise paste
//     from clipboard. Prevents the browser's default context menu.
//   - Ctrl+V: handled by xterm's native paste path (native `paste`
//     event on the textarea); we intercept to sanitize the data
//     before handing it to the PTY.
//   - Ctrl+C with selection: copy + clear selection. Ctrl+C with no
//     selection: passes through as SIGINT to the shell (default xterm
//     behavior). This is how Windows Terminal does "copy AND kill".
//
// URL linkification via `@xterm/addon-web-links`. GPU rendering via
// `@xterm/addon-webgl` with graceful fallback on context loss.

import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";
import { useEffect, useRef, useState } from "react";

import { connectPty, type ConnectionState } from "../api/ws";
import { uploadRepoFile } from "../api/client";
import { useAppCommand } from "../state/AppCommands";
import { useSessions } from "../state/SessionStore";

/** Explicit session-lifecycle discriminated union. The previous shape
 * used `number | null | undefined` where undefined = "haven't heard",
 * null = "dead, no code", number = "dead, code" — readable only with
 * a code-comment next to the state. With an explicit kind tag the
 * rendering condition becomes `exit.kind === "dead"` instead of
 * "!== undefined", which didn't spell out what it meant. */
type ExitStatus =
  | { kind: "alive" }
  | { kind: "dead"; code: number | null };
import { copyToClipboard, readClipboard, sanitizePaste } from "./terminal/clipboard";
import "@xterm/xterm/css/xterm.css";
import "./TerminalPane.css";

const PASTE_AS_FILE_BYTES = 4 * 1024;
const PASTE_AS_FILE_LINES = 200;

export function TerminalPane({ sessionId }: { sessionId: string }) {
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const [connState, setConnState] = useState<ConnectionState>("connecting");
  const [exitStatus, setExitStatus] = useState<ExitStatus>({ kind: "alive" });
  const sessions = useSessions((store) => store.sessions);
  const repoName = sessions.find((s) => s.id === sessionId)?.repo ?? null;
  const repoRef = useRef<string | null>(repoName);
  repoRef.current = repoName;

  useEffect(() => {
    if (!hostRef.current) return;
    const host = hostRef.current;

    const term = new Terminal({
      // 5000 lines of live scrollback. The original `scrollback: 0`
      // conflated two different concerns: "don't replay historical
      // ANSI into a linear buffer" (correct, and why the timeline
      // exists) with "don't buffer the current session at all"
      // (wrong — loses routine shell output like `npm install`,
      // test runs, diffs). Alt-screen TUIs (claude, vim) stay in
      // the viewport untouched because xterm.js handles alt-screen
      // separately from the main scrollback.
      scrollback: 5000,
      cursorBlink: true,
      fontFamily:
        "ui-monospace, SFMono-Regular, Consolas, 'Liberation Mono', monospace",
      fontSize: 13,
      theme: {
        background: "#0b0d12",
        foreground: "#e5e7eb",
        cursor: "#93c5fd",
        selectionBackground: "#1e40af",
        black: "#1f232b",
        red: "#f87171",
        green: "#86efac",
        yellow: "#fde68a",
        blue: "#93c5fd",
        magenta: "#f0abfc",
        cyan: "#67e8f9",
        white: "#e5e7eb",
        brightBlack: "#4b5563",
        brightRed: "#fca5a5",
        brightGreen: "#bbf7d0",
        brightYellow: "#fef3c7",
        brightBlue: "#bfdbfe",
        brightMagenta: "#f5d0fe",
        brightCyan: "#a5f3fc",
        brightWhite: "#f9fafb",
      },
    });
    termRef.current = term;

    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(new WebLinksAddon());
    term.open(host);

    // WebGL renderer for sharper text + lower latency. Falls back to
    // the default DOM renderer if the GPU context is lost (tab
    // backgrounded, VM without acceleration, etc).
    let webgl: WebglAddon | null = null;
    try {
      webgl = new WebglAddon();
      webgl.onContextLoss(() => {
        webgl?.dispose();
        webgl = null;
      });
      term.loadAddon(webgl);
    } catch {
      // WebGL unsupported — default renderer is still correct.
    }

    requestAnimationFrame(() => {
      try {
        fit.fit();
      } catch {
        // Happy-dom or pre-layout — the ResizeObserver will fire again.
      }
    });

    const conn = connectPty(sessionId, {
      onBytes: (chunk) => term.write(chunk),
      onServerMsg: (msg) => {
        if (msg.t === "dead") setExitStatus({ kind: "dead", code: msg.exit ?? null });
      },
      onConnectionChange: setConnState,
    });

    const onData = term.onData((data) => conn.sendInput(data));

    // Key handler: intercept Ctrl+C (copy/SIGINT split) and Ctrl+V
    // family (paste). Without intercepting Ctrl+V, xterm's default is
    // to send \x16 (SYN) to the PTY as a control byte AND call
    // preventDefault on the keydown — which stops the browser from
    // firing the native `paste` event, so our paste listener never
    // sees it. Returning false here tells xterm to skip its default
    // processing (no preventDefault), letting the browser fire paste
    // normally. The textarea paste listener below then handles it once.
    //
    // Ctrl+Shift+V: xterm has its own built-in paste binding that
    // programmatically reads navigator.clipboard and calls term.paste.
    // The browser ALSO dispatches a native paste event on this combo.
    // That's two paste paths firing. Returning false kills xterm's
    // binding, leaving only the native-event path we already handle.
    term.attachCustomKeyEventHandler((ev: KeyboardEvent): boolean => {
      if (ev.type !== "keydown") return true;
      const ctrl = ev.ctrlKey && !ev.metaKey && !ev.altKey;
      if (!ctrl) return true;

      // Ctrl+C with selection → copy; without selection → let default
      // through so the shell gets SIGINT. Matches Windows Terminal.
      if (!ev.shiftKey && (ev.key === "c" || ev.key === "C")) {
        const sel = term.getSelection();
        if (sel.length > 0) {
          void copyToClipboard(sel);
          term.clearSelection();
          return false;
        }
        return true;
      }

      // Ctrl+V (with or without shift): yield to the native paste event.
      if (ev.key === "v" || ev.key === "V") {
        return false;
      }

      return true;
    });

    // Intercept the textarea's paste event so we can sanitize
    // zero-width chars and CRLF before the data hits the PTY. xterm's
    // own paste handling reads from the same event; preventing default
    // + calling term.paste(clean) routes through its bracketed-paste
    // wrapper if the shell has enabled it.
    const textarea: HTMLTextAreaElement | null = term.textarea ?? null;
    const onPaste = (ev: Event) => {
      const ce = ev as ClipboardEvent;
      if (!ce.clipboardData) return;
      ev.preventDefault();
      const raw = ce.clipboardData.getData("text/plain");
      const lineCount = (raw.match(/\n/g)?.length ?? 0) + 1;
      const large =
        raw.length > PASTE_AS_FILE_BYTES || lineCount > PASTE_AS_FILE_LINES;
      if (large && repoRef.current) {
        // Intercept: offer paste-as-file to avoid choking the PTY. The
        // confirm() is blunt but this is a rare path.
        const accept = window.confirm(
          `Clipboard is ${raw.length} bytes / ${lineCount} lines — paste as a file instead?\n\n` +
            "OK  = save to .sulion-paste/ and inject the path\n" +
            "Cancel = paste the raw contents inline",
        );
        if (accept) {
          const ts = new Date()
            .toISOString()
            .replace(/[:.]/g, "-")
            .replace("T", "_");
          const filename = `paste-${ts}.txt`;
          const blob = new File([raw], filename, { type: "text/plain" });
          void uploadRepoFile(
            repoRef.current,
            ".sulion-paste",
            blob,
          )
            .then((res) => {
              term.paste(res.path + " ");
            })
            .catch(() => {
              // On failure, fall back to an inline paste.
              term.paste(sanitizePaste(raw));
            });
          return;
        }
      }
      term.paste(sanitizePaste(raw));
    };
    textarea?.addEventListener("paste", onPaste);

    // Right-click: Windows-Terminal-style copy/paste.
    const onContextMenu: EventListener = (ev) => {
      ev.preventDefault();
      void (async () => {
        const sel = term.getSelection();
        if (sel.length > 0) {
          await copyToClipboard(sel);
          term.clearSelection();
          return;
        }
        const text = await readClipboard();
        if (text != null) {
          term.paste(sanitizePaste(text));
        }
        // If readClipboard returned null (HTTP context), silently do
        // nothing — the user can still paste via Ctrl+V, which goes
        // through the native paste event and works on HTTP.
      })();
    };
    host.addEventListener("contextmenu", onContextMenu);

    const resize = () => {
      try {
        fit.fit();
        conn.sendResize(term.cols, term.rows);
      } catch {
        // Host not laid out yet; the ResizeObserver will fire again.
      }
    };
    let ro: ResizeObserver | null = null;
    if (typeof ResizeObserver !== "undefined") {
      ro = new ResizeObserver(resize);
      ro.observe(host);
    } else {
      window.addEventListener("resize", resize);
    }
    resize();

    return () => {
      ro?.disconnect();
      if (!ro) window.removeEventListener("resize", resize);
      textarea?.removeEventListener("paste", onPaste);
      host.removeEventListener("contextmenu", onContextMenu);
      onData.dispose();
      conn.close();
      webgl?.dispose();
      term.dispose();
      termRef.current = null;
    };
  }, [sessionId]);

  useAppCommand("inject-terminal", ({ sessionId: targetSessionId, text }) => {
    if (targetSessionId !== sessionId) return;
    termRef.current?.paste(sanitizePaste(text));
  });

  return (
    <div className="terminal-pane" data-testid="terminal-pane">
      <div ref={hostRef} className="terminal-pane__host" />
      {connState !== "open" && (
        <div className="terminal-pane__status">
          {connState === "connecting" && "connecting…"}
          {connState === "reconnecting" && "reconnecting…"}
          {connState === "closed" && "closed"}
        </div>
      )}
      {exitStatus.kind === "dead" && (
        <div className="terminal-pane__banner">
          shell exited{exitStatus.code == null ? "" : ` with code ${exitStatus.code}`} —
          session no longer receiving input
        </div>
      )}
    </div>
  );
}
