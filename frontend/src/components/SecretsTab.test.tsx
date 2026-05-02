import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { SecretEnvelope, SecretMetadata } from "../api/types";
import { SecretsTab } from "./SecretsTab";

interface SecretRecord {
  metadata: SecretMetadata;
  envelope: SecretEnvelope;
}

function installSecretFetchMock(initial: SecretRecord[] = []) {
  const records = new Map(initial.map((item) => [item.metadata.id, item]));
  const requests: Array<{ url: string; method: string; body: unknown }> = [];

  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo, init?: RequestInit) => {
      const url = typeof input === "string" ? input : input.url;
      const method = init?.method ?? "GET";
      const body = init?.body ? JSON.parse(init.body as string) : null;
      requests.push({ url, method, body });

      if (url === "/broker/v1/secrets" && method === "GET") {
        return jsonResponse(Array.from(records.values()).map((record) => record.metadata));
      }

      const match = url.match(/^\/broker\/v1\/secrets\/([^/]+)$/);
      if (!match) {
        return new Response(JSON.stringify({ error: "not found" }), { status: 404 });
      }
      const id = decodeURIComponent(match[1]!);

      if (method === "GET") {
        const record = records.get(id);
        if (!record) {
          return new Response(JSON.stringify({ error: "secret not found" }), { status: 404 });
        }
        return jsonResponse(record.envelope);
      }

      if (method === "PUT") {
        const envelope = body as SecretEnvelope;
        records.set(id, {
          metadata: {
            id,
            description: envelope.description,
            scope: envelope.scope,
            repo: envelope.repo,
            env_keys: Object.keys(envelope.env).sort(),
            updated_at: "2026-04-24T00:00:00Z",
          },
          envelope: {
            ...envelope,
            env: Object.fromEntries(Object.keys(envelope.env).map((key) => [key, ""])),
          },
        });
        return new Response(null, { status: 201, statusText: "Created" });
      }

      if (method === "DELETE") {
        records.delete(id);
        return new Response(null, { status: 204 });
      }

      return new Response(JSON.stringify({ error: "not found" }), { status: 404 });
    }),
  );

  return { records, requests };
}

const jsonResponse = (body: unknown) =>
  new Response(JSON.stringify(body), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });

describe("SecretsTab", () => {
  it("creates an explicit key/value bundle and handles empty 201 responses", async () => {
    const server = installSecretFetchMock();
    const user = userEvent.setup();
    render(<SecretsTab />);

    await screen.findByText("No secrets yet.");

    await user.clear(screen.getByLabelText("ID"));
    await user.type(screen.getByLabelText("ID"), "claude-api");
    await user.type(screen.getByLabelText("Description"), "Claude API key");
    const keyInput = screen.getByDisplayValue("EXAMPLE_KEY");
    fireEvent.change(keyInput, { target: { value: "ANTHROPIC_API_KEY" } });
    await user.type(screen.getByPlaceholderText("value"), "sulion-secret-value");
    await user.click(screen.getByRole("button", { name: "Save" }));

    await screen.findByText("Saved claude-api");
    expect(screen.getAllByText("claude-api").length).toBeGreaterThan(0);
    await screen.findByText("ANTHROPIC_API_KEY");

    const put = server.requests.find(
      (request) => request.url === "/broker/v1/secrets/claude-api" && request.method === "PUT",
    );
    expect(put?.body).toEqual({
      description: "Claude API key",
      scope: "global",
      repo: null,
      env: { ANTHROPIC_API_KEY: "sulion-secret-value" },
    });
  });

  it("does not display existing secret values and sends blank values to preserve them", async () => {
    const server = installSecretFetchMock([
      {
        metadata: {
          id: "claude-api",
          description: "Claude",
          scope: "global",
          repo: null,
          env_keys: ["ANTHROPIC_API_KEY"],
          updated_at: "2026-04-24T00:00:00Z",
        },
        envelope: {
          description: "Claude",
          scope: "global",
          repo: null,
          env: { ANTHROPIC_API_KEY: "" },
        },
      },
    ]);
    const user = userEvent.setup();
    render(<SecretsTab />);

    await screen.findByText("Blank existing values are kept; enter a value to overwrite.");
    expect(screen.queryByDisplayValue("sulion-secret-value")).toBeNull();
    expect((screen.getByPlaceholderText("keep existing") as HTMLInputElement).value).toBe("");

    fireEvent.change(screen.getByLabelText("Description"), {
      target: { value: "Claude rotated elsewhere" },
    });
    await user.click(screen.getByRole("button", { name: "Save" }));

    await screen.findByText("Saved claude-api");
    await waitFor(() => {
      const put = server.requests
        .filter(
          (request) =>
            request.url === "/broker/v1/secrets/claude-api" && request.method === "PUT",
        )
        .at(-1);
      expect(put?.body).toEqual({
        description: "Claude rotated elsewhere",
        scope: "global",
        repo: null,
        env: { ANTHROPIC_API_KEY: "" },
      });
    });
  });
});
