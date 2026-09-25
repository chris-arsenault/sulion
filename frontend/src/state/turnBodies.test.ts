import { afterEach, describe, expect, it, vi } from "vitest";
import * as api from "../api/turnStream";
import { requestBody } from "./turnBodies";

afterEach(() => vi.restoreAllMocks());
describe("tool body batching", () => {
  it("shares a bounded request for simultaneously opened tools", async () => {
    const read = vi.spyOn(api, "getOperationBodies").mockImplementation(async (_s, _t, ids) => ids.map((id) => ({
      id, generation: "g", body_version: 1, input: {}, result: null,
    })));
    const controller = new AbortController();
    const bodies = await Promise.all(Array.from({ length: 20 }, (_, i) => requestBody("s", 1, `${i}`, controller.signal)));
    expect(read).toHaveBeenCalledTimes(2);
    expect(read.mock.calls.map((call) => call[2].length)).toEqual([16, 4]);
    expect(bodies.map((body) => body.id)).toEqual(Array.from({ length: 20 }, (_, i) => `${i}`));
  });
});
