import { describe, expect, it } from "vitest";
import { consumeTurnStream, type TurnRecord } from "./turnStream";

describe("turn wire stream", () => {
  it("delivers records before completion and decodes fragmented UTF-8", async () => {
    let writer!: ReadableStreamDefaultController<Uint8Array>;
    const stream = new ReadableStream<Uint8Array>({ start(controller) { writer = controller; } });
    const records: TurnRecord[] = [];
    const read = consumeTurnStream(new Response(stream), new AbortController().signal, (record) => records.push(record));
    const header = new TextEncoder().encode(JSON.stringify({ kind: "header", turn: { preview: "早い" } }) + "\n");
    for (const byte of header) writer.enqueue(new Uint8Array([byte]));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(records).toHaveLength(1);
    expect(records[0]).toMatchObject({ turn: { preview: "早い" } });
    writer.enqueue(new TextEncoder().encode('{"kind":"batch","items":[{"offset":1}],"operations":[],"cursor":{}}\n'));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(records.map((record) => record.kind)).toEqual(["header", "batch"]);
    writer.enqueue(new TextEncoder().encode('{"kind":"complete","cursor":{}}\n'));
    await read;
    expect(records.map((record) => record.kind)).toEqual(["header", "batch", "complete"]);
  });

  it("rejects a truncated transfer without inventing a completed cursor", async () => {
    const records: TurnRecord[] = [];
    await expect(consumeTurnStream(new Response('{"kind":"header"}\n{"kind":"bat'),
      new AbortController().signal, (record) => records.push(record))).rejects.toThrow("interrupted");
    expect(records).toEqual([{ kind: "header" }]);
  });

  it("surfaces server errors and explicit rebuild resets", async () => {
    await expect(consumeTurnStream(new Response('{"kind":"error","message":"failed"}\n'),
      new AbortController().signal, () => {})).rejects.toThrow("failed");
    const records: TurnRecord[] = [];
    await consumeTurnStream(new Response('{"kind":"reset"}\n'), new AbortController().signal,
      (record) => records.push(record));
    expect(records).toEqual([{ kind: "reset" }]);
  });
});
