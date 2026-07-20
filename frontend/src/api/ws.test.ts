import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { authFetch } from "./client";
import { connectPty } from "./ws";

vi.mock("./client", () => ({ authFetch: vi.fn() }));

const authFetchMock = vi.mocked(authFetch);

type Listener = (event: Event | MessageEvent) => void;

class MockWebSocket {
  static readonly OPEN = 1;
  static instances: MockWebSocket[] = [];

  readonly url: string;
  readonly protocols: string[];
  binaryType: BinaryType = "blob";
  readyState = 0;
  sent: string[] = [];
  private listeners = new Map<string, Listener[]>();

  constructor(url: string | URL, protocols?: string | string[]) {
    this.url = String(url);
    this.protocols = typeof protocols === "string" ? [protocols] : (protocols ?? []);
    MockWebSocket.instances.push(this);
  }

  addEventListener(type: string, listener: Listener) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }

  send(data: string) {
    this.sent.push(data);
  }

  close() {
    this.readyState = 3;
    this.emit("close", new Event("close"));
  }

  serverOpen() {
    this.readyState = MockWebSocket.OPEN;
    this.emit("open", new Event("open"));
  }

  serverClose() {
    this.readyState = 3;
    this.emit("close", new Event("close"));
  }

  private emit(type: string, event: Event | MessageEvent) {
    for (const listener of this.listeners.get(type) ?? []) listener(event);
  }
}

const ticketResponse = (ticket: string) =>
  Promise.resolve(
    new Response(JSON.stringify({ ticket, expires_in: 30 }), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    }),
  );

const flushPromises = async () => {
  for (let step = 0; step < 6; step += 1) await Promise.resolve();
};

describe("connectPty", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    MockWebSocket.instances = [];
    authFetchMock.mockReset();
    vi.stubGlobal("WebSocket", MockWebSocket as unknown as typeof WebSocket);
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("uses a one-use ticket in the WebSocket protocol instead of the URL", async () => {
    authFetchMock.mockImplementation(() => ticketResponse("ticket-one"));

    const connection = connectPty("session-1", { onBytes: vi.fn() });
    await flushPromises();

    expect(authFetchMock).toHaveBeenCalledWith("/api/ws-tickets", {
      method: "POST",
      body: JSON.stringify({ session_id: "session-1" }),
    });
    expect(MockWebSocket.instances).toHaveLength(1);
    const websocketProtocol = window.location.protocol === "https:" ? "wss" : "ws";
    expect(MockWebSocket.instances[0].url).toBe(
      `${websocketProtocol}://${window.location.host}/ws/sessions/session-1`,
    );
    expect(MockWebSocket.instances[0].protocols).toEqual([
      "sulion.v1",
      "sulion.ticket.ticket-one",
    ]);
    connection.close();
  });

  it("sends heartbeats and obtains a fresh ticket after reconnect", async () => {
    authFetchMock
      .mockImplementationOnce(() => ticketResponse("ticket-one"))
      .mockImplementationOnce(() => ticketResponse("ticket-two"));

    const connection = connectPty("session-1", { onBytes: vi.fn() });
    await flushPromises();
    const first = MockWebSocket.instances[0];
    first.serverOpen();

    await vi.advanceTimersByTimeAsync(25_000);
    expect(first.sent).toContain(JSON.stringify({ t: "ping" }));

    first.serverClose();
    await vi.advanceTimersByTimeAsync(250);
    await flushPromises();

    expect(MockWebSocket.instances).toHaveLength(2);
    expect(MockWebSocket.instances[1].protocols).toContain("sulion.ticket.ticket-two");
    expect(authFetchMock).toHaveBeenCalledTimes(2);
    connection.close();
  });
});
