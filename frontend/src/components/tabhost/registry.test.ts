import { beforeEach, describe, expect, it } from "vitest";

import { claimHost, hostFor, releaseHost, resetTabHostRegistry } from "./registry";

describe("tab host registry", () => {
  beforeEach(() => resetTabHostRegistry());

  it("returns a stable host element per tab id", () => {
    const a = hostFor("t1");
    expect(hostFor("t1")).toBe(a);
    expect(hostFor("t2")).not.toBe(a);
  });

  it("parents the host into the claimant and detaches on release", () => {
    const container = document.createElement("div");
    const unclaim = claimHost("t1", container, 1);
    expect(hostFor("t1").parentElement).toBe(container);
    unclaim();
    expect(hostFor("t1").parentElement).toBeNull();
  });

  it("a higher-priority claim takes the host and hands it back on release", () => {
    const pane = document.createElement("div");
    const modal = document.createElement("div");
    claimHost("t1", pane, 1);
    const unclaimModal = claimHost("t1", modal, 2);
    expect(hostFor("t1").parentElement).toBe(modal);
    unclaimModal();
    expect(hostFor("t1").parentElement).toBe(pane);
  });

  it("equal priority: the most recent claim wins", () => {
    const first = document.createElement("div");
    const second = document.createElement("div");
    claimHost("t1", first, 1);
    claimHost("t1", second, 1);
    expect(hostFor("t1").parentElement).toBe(second);
  });

  it("releaseHost forgets the element entirely", () => {
    const container = document.createElement("div");
    claimHost("t1", container, 1);
    const el = hostFor("t1");
    releaseHost("t1");
    expect(el.parentElement).toBeNull();
    expect(hostFor("t1")).not.toBe(el);
  });
});
