// WebSocket client for the PTY attach endpoint. Handles reconnect with
// exponential backoff. Callers get raw bytes (Uint8Array) for binary
// frames and typed ServerMsg for JSON text frames.

import { authFetch } from "./client";

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
const HEARTBEAT_INTERVAL_MS = 25_000;
const WS_PROTOCOL = "sulion.v1";
const WS_TICKET_PROTOCOL_PREFIX = "sulion.ticket.";

type WsTicketResponse = {
  ticket: string;
  expires_in: number;
};

async function issueWsTicket(sessionId: string): Promise<string> {
  const response = await authFetch("/api/ws-tickets", {
    method: "POST",
    body: JSON.stringify({ session_id: sessionId }),
  });
  if (!response.ok) throw new Error(`WebSocket ticket request failed: ${response.status}`);
  const body = (await response.json()) as WsTicketResponse;
  if (!body.ticket) throw new Error("WebSocket ticket response was empty");
  return body.ticket;
}

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
  let heartbeatTimer: ReturnType<typeof setInterval> | null = null;
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

  const stopHeartbeat = () => {
    if (heartbeatTimer) {
      clearInterval(heartbeatTimer);
      heartbeatTimer = null;
    }
  };

  const scheduleReconnect = () => {
    if (closed || reconnectTimer) return;
    setState("reconnecting");
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      backoffMs = Math.min(backoffMs * 2, MAX_BACKOFF_MS);
      void open();
    }, backoffMs);
  };

  const open = async () => {
    if (closed) return;
    setState(connectionState === "closed" ? "connecting" : connectionState);
    let ticket: string;
    try {
      ticket = await issueWsTicket(sessionId);
    } catch {
      scheduleReconnect();
      return;
    }
    if (closed) return;
    socket = new WebSocket(url(), [WS_PROTOCOL, `${WS_TICKET_PROTOCOL_PREFIX}${ticket}`]);
    socket.binaryType = "arraybuffer";

    socket.addEventListener("open", () => {
      backoffMs = INITIAL_BACKOFF_MS;
      setState("open");
      stopHeartbeat();
      heartbeatTimer = setInterval(() => {
        if (socket?.readyState === WebSocket.OPEN) {
          socket.send(JSON.stringify({ t: "ping" }));
        }
      }, HEARTBEAT_INTERVAL_MS);
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
      stopHeartbeat();
      if (closed) {
        setState("closed");
        return;
      }
      scheduleReconnect();
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
    stopHeartbeat();
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

  void open();

  return {
    sendInput,
    sendResize,
    close,
    state: () => connectionState,
  };
}
