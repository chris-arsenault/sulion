import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// xterm.js depends on canvas/layout APIs that happy-dom doesn't fully
// implement. We mock just enough to verify the component wires up the
// WebSocket + clipboard hooks.

const writes: Array<string | Uint8Array> = [];
const dataHandlers: Array<(d: string) => void> = [];
let keyHandler: ((e: KeyboardEvent) => boolean) | null = null;
const pasted: string[] = [];
const cleared: number[] = [];
let currentSelection = "";

const mockTerm = {
  options: { fontSize: 13 },
  loadAddon: vi.fn(),
  open: vi.fn(),
  write: vi.fn((chunk: string | Uint8Array) => writes.push(chunk)),
  onData: vi.fn((handler: (d: string) => void) => {
    dataHandlers.push(handler);
    return { dispose: vi.fn() };
  }),
  attachCustomKeyEventHandler: vi.fn((h: (e: KeyboardEvent) => boolean) => {
    keyHandler = h;
  }),
  paste: vi.fn((text: string) => pasted.push(text)),
  focus: vi.fn(),
  getSelection: vi.fn(() => currentSelection),
  clearSelection: vi.fn(() => {
    cleared.push(Date.now());
    currentSelection = "";
  }),
  textarea: document.createElement("textarea"),
  cols: 80,
  rows: 24,
  dispose: vi.fn(),
};

vi.mock("@xterm/xterm", () => ({
  Terminal: vi.fn(() => mockTerm),
}));
vi.mock("@xterm/addon-fit", () => ({
  FitAddon: vi.fn(() => ({ fit: vi.fn() })),
}));
vi.mock("@xterm/addon-web-links", () => ({
  WebLinksAddon: vi.fn(() => ({})),
}));
vi.mock("@xterm/addon-webgl", () => ({
  WebglAddon: vi.fn(() => ({
    onContextLoss: vi.fn(),
    dispose: vi.fn(),
  })),
}));
vi.mock("@xterm/xterm/css/xterm.css", () => ({}));

const mockConn = {
  sendInput: vi.fn(),
  sendResize: vi.fn(),
  close: vi.fn(),
  state: vi.fn(() => "open"),
};
const connectPtyMock = vi.fn();
vi.mock("../api/ws", () => ({
  connectPty: (...args: Parameters<typeof connectPtyMock>) => connectPtyMock(...args),
}));

import { appCommands } from "../state/AppCommands";
import { TerminalPane } from "./TerminalPane";
import { appStatePayload, jsonResponse } from "../test/appState";

describe("TerminalPane", () => {
  beforeEach(() => {
    window.localStorage.clear();
    writes.length = 0;
    dataHandlers.length = 0;
    pasted.length = 0;
    cleared.length = 0;
    currentSelection = "";
    keyHandler = null;
    mockTerm.options.fontSize = 13;
    vi.clearAllMocks();
    connectPtyMock.mockImplementation((_sessionId, handlers) => {
      (connectPtyMock as unknown as { last: unknown }).last = handlers;
      return mockConn;
    });
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo) => {
        const url = typeof input === "string" ? input : (input as Request).url;
        if (url === "/api/app-state") return jsonResponse(appStatePayload());
        return new Response("", { status: 404 });
      }),
    );
  });
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("wires WebSocket bytes into term.write", async () => {
    render(<TerminalPane sessionId="abc" />);
    await waitFor(() => {
      expect(connectPtyMock).toHaveBeenCalled();
    });
    const handlers = (connectPtyMock as unknown as { last: { onBytes: (c: Uint8Array) => void } })
      .last;
    handlers.onBytes(new Uint8Array([104, 105])); // "hi"
    expect(mockTerm.write).toHaveBeenCalled();
    expect(writes[0]).toEqual(new Uint8Array([104, 105]));
  });

  it("routes xterm keystrokes into conn.sendInput", async () => {
    render(<TerminalPane sessionId="abc" />);
    await waitFor(() => expect(dataHandlers.length).toBeGreaterThan(0));
    dataHandlers[0]!("ls\n");
    expect(mockConn.sendInput).toHaveBeenCalledWith("ls\n");
  });

  it("shows exit banner when the server reports dead", async () => {
    const { findByText } = render(<TerminalPane sessionId="abc" />);
    await waitFor(() => expect(connectPtyMock).toHaveBeenCalled());
    const handlers = (connectPtyMock as unknown as { last: { onServerMsg: (m: unknown) => void } })
      .last;
    act(() => {
      handlers.onServerMsg({ t: "dead", exit: 0 });
    });
    expect(await findByText(/shell exited/)).toBeDefined();
  });

  it("remounts on sessionId change; idle re-renders do not reconnect", async () => {
    const { rerender } = render(<TerminalPane sessionId="a" />);
    await waitFor(() => expect(connectPtyMock).toHaveBeenCalledTimes(1));
    rerender(<TerminalPane sessionId="a" />);
    expect(connectPtyMock).toHaveBeenCalledTimes(1);
    rerender(<TerminalPane sessionId="b" />);
    await waitFor(() => expect(connectPtyMock).toHaveBeenCalledTimes(2));
  });

  it("Ctrl+C with a selection copies and cancels; no passthrough", async () => {
    render(<TerminalPane sessionId="abc" />);
    await waitFor(() => expect(keyHandler).not.toBeNull());
    currentSelection = "selected text";
    const ev = new KeyboardEvent("keydown", { key: "c", ctrlKey: true });
    const result = keyHandler!(ev);
    expect(result).toBe(false);
    expect(mockTerm.clearSelection).toHaveBeenCalled();
  });

  it("Ctrl+C without a selection passes through (returns true)", async () => {
    render(<TerminalPane sessionId="abc" />);
    await waitFor(() => expect(keyHandler).not.toBeNull());
    currentSelection = "";
    const ev = new KeyboardEvent("keydown", { key: "c", ctrlKey: true });
    expect(keyHandler!(ev)).toBe(true);
    expect(mockTerm.clearSelection).not.toHaveBeenCalled();
  });

  it("paste event sanitizes before calling term.paste", async () => {
    render(<TerminalPane sessionId="abc" />);
    await waitFor(() => expect(connectPtyMock).toHaveBeenCalled());

    const clipboardData = {
      getData: vi.fn(() => "hello\u200Bworld\r\nend"),
    };
    const pasteEvent = new Event("paste") as Event & {
      clipboardData: typeof clipboardData;
    };
    (pasteEvent as unknown as { clipboardData: typeof clipboardData }).clipboardData =
      clipboardData;
    mockTerm.textarea.dispatchEvent(pasteEvent);

    expect(mockTerm.paste).toHaveBeenCalledWith("helloworld\nend");
    expect(mockTerm.paste).toHaveBeenCalledTimes(1);
  });

  it("pastes injected terminal text from the app command layer", async () => {
    render(<TerminalPane sessionId="abc" />);
    await waitFor(() => expect(connectPtyMock).toHaveBeenCalled());

    appCommands.injectTerminal({ sessionId: "abc", text: "hello\u200Bworld\r\nend" });

    expect(mockTerm.paste).toHaveBeenCalledWith("helloworld\nend");
    expect(mockTerm.focus).toHaveBeenCalled();
  });

  it("right-click with selection copies and cancels context menu", async () => {
    const { container } = render(<TerminalPane sessionId="abc" />);
    await waitFor(() => expect(connectPtyMock).toHaveBeenCalled());

    currentSelection = "copy-me";
    // Writing to the host element
    const host = container.querySelector(".terminal-pane__host") as HTMLElement;
    const ev = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    const preventDefault = vi.spyOn(ev, "preventDefault");
    host.dispatchEvent(ev);

    // contextmenu handler is async — wait a tick
    await new Promise((r) => setTimeout(r, 0));
    expect(preventDefault).toHaveBeenCalled();
    expect(mockTerm.clearSelection).toHaveBeenCalled();
  });

  it("adjusts terminal text size without reconnecting", async () => {
    const user = userEvent.setup();
    render(<TerminalPane sessionId="abc" />);
    await waitFor(() => expect(connectPtyMock).toHaveBeenCalledTimes(1));

    await user.click(screen.getByRole("button", { name: /increase terminal text size/i }));

    expect(mockTerm.options.fontSize).toBe(14);
    expect(window.localStorage.getItem("sulion.terminal.font-size.v1")).toBe("14");
    expect(connectPtyMock).toHaveBeenCalledTimes(1);
  });
});
