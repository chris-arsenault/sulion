// WebSocket client for the PTY attach endpoint. Handles reconnect with
// exponential backoff. Callers get raw bytes (Uint8Array) for binary
// frames and typed ServerMsg for JSON text frames.

export interface ServerReady {
  t: "ready";
}
export interface ServerDead {
  t: "dead";
  exit: number | null;
}
export interface ServerPong {
  t: "pong";
}
export interface ServerError {
  t: "error";
  message: string;
}
export type ServerMsg = ServerReady | ServerDead | ServerPong | ServerError;

export interface PtyConnectionHandlers {
  onBytes: (chunk: Uint8Array) => void;
  onServerMsg?: (msg: ServerMsg) => void;
  onConnectionChange?: (state: ConnectionState) => void;
}

export type ConnectionState = "connecting" | "open" | "reconnecting" | "closed";

export interface PtyConnection {
  sendInput: (data: string) => void;
  sendResize: (cols: number, rows: number) => void;
  close: () => void;
  state: () => ConnectionState;
}

const INITIAL_BACKOFF_MS = 250;
const MAX_BACKOFF_MS = 10_000;

/** Opens a connection to /ws/sessions/:id. Reconnects automatically until
 * `close()` is called. */
export function connectPty(
  sessionId: string,
  handlers: PtyConnectionHandlers,
): PtyConnection {
  let socket: WebSocket | null = null;
  let backoffMs = INITIAL_BACKOFF_MS;
  let closed = false;
  let connectionState: ConnectionState = "connecting";
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  let pendingResize: { cols: number; rows: number } | null = null;

  const setState = (s: ConnectionState) => {
    if (connectionState === s) return;
    connectionState = s;
    handlers.onConnectionChange?.(s);
  };

  const url = () => {
    const proto = window.location.protocol === "https:" ? "wss" : "ws";
    return `${proto}://${window.location.host}/ws/sessions/${sessionId}`;
  };

  const open = () => {
    if (closed) return;
    setState(connectionState === "closed" ? "connecting" : connectionState);
    socket = new WebSocket(url());
    socket.binaryType = "arraybuffer";

    socket.addEventListener("open", () => {
      backoffMs = INITIAL_BACKOFF_MS;
      setState("open");
      if (pendingResize) {
        const { cols, rows } = pendingResize;
        pendingResize = null;
        sendResize(cols, rows);
      }
    });

    socket.addEventListener("message", (ev) => {
      if (typeof ev.data === "string") {
        try {
          const parsed = JSON.parse(ev.data) as ServerMsg;
          handlers.onServerMsg?.(parsed);
        } catch {
          // Ignore malformed — server will never send these, but be robust.
        }
      } else if (ev.data instanceof ArrayBuffer) {
        handlers.onBytes(new Uint8Array(ev.data));
      } else if (ev.data instanceof Blob) {
        ev.data.arrayBuffer().then((buf) => handlers.onBytes(new Uint8Array(buf)));
      }
    });

    const onDisconnect = () => {
      if (closed) {
        setState("closed");
        return;
      }
      setState("reconnecting");
      reconnectTimer = setTimeout(() => {
        reconnectTimer = null;
        backoffMs = Math.min(backoffMs * 2, MAX_BACKOFF_MS);
        open();
      }, backoffMs);
    };
    socket.addEventListener("close", onDisconnect);
    socket.addEventListener("error", () => {
      // Let the close handler drive reconnection.
      try {
        socket?.close();
      } catch {
        // Ignore.
      }
    });
  };

  const sendInput = (data: string) => {
    if (socket && socket.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify({ t: "input", data }));
    }
  };

  const sendResize = (cols: number, rows: number) => {
    if (socket && socket.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify({ t: "resize", cols, rows }));
    } else {
      pendingResize = { cols, rows };
    }
  };

  const close = () => {
    closed = true;
    if (reconnectTimer) {
      clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
    try {
      socket?.close();
    } catch {
      // Ignore.
    }
    setState("closed");
  };

  open();

  return {
    sendInput,
    sendResize,
    close,
    state: () => connectionState,
  };
}
