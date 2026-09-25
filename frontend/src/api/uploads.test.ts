import { afterEach, describe, expect, it, vi } from "vitest";
import { createUpload, putStagedFile, uploadFile } from "./client";

const target = { repo: "repo", workspaceId: "workspace" };
const grant = { url: "https://bucket.s3.us-east-1.amazonaws.com/uploads/id?signature=secret",
  headers: { "if-none-match": "*", "x-amz-meta-upload-binding": "binding" }, expires_in: 900 };
function json(value: unknown) { return new Response(JSON.stringify(value)); }
afterEach(() => { vi.unstubAllGlobals(); window.__APP_CONFIG__ = {}; });

describe("upload transport", () => {
  it("uses the existing workspace multipart endpoint on LAN", async () => {
    window.__APP_CONFIG__ = { publicUploadOrigin: "https://sulion.services.ahara.io" };
    const fetch = vi.fn().mockResolvedValue(json({ path: "/workspace/file", size: 3 }));
    vi.stubGlobal("fetch", fetch);
    await uploadFile(target, ".sulion-paste", new File(["abc"], "file"));
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(fetch.mock.calls[0][0]).toBe("/api/workspaces/workspace/upload?path=.sulion-paste");
    expect(fetch.mock.calls[0][1].body).toBeInstanceOf(FormData);
  });

  it("sends only metadata to Sulion, raw bytes to S3, and waits for installation", async () => {
    window.__APP_CONFIG__ = { publicUploadOrigin: window.location.origin };
    let installed!: (value: Response) => void;
    const fetch = vi.fn().mockResolvedValueOnce(json({ id: "upload-id", grant }))
      .mockResolvedValueOnce(new Response(null))
      .mockImplementationOnce(() => new Promise<Response>((resolve) => { installed = resolve; }));
    vi.stubGlobal("fetch", fetch);
    const file = new File(["abc"], "file");
    let done = false;
    const upload = uploadFile(target, ".sulion-paste", file).then((result) => { done = true; return result; });
    await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(3));
    expect(done).toBe(false);
    const input = JSON.parse(fetch.mock.calls[0][1].body);
    expect(input).toEqual({ workspace_id: "workspace", directory: ".sulion-paste", filename: "file",
      size: 3, checksum: "ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0=" });
    expect(fetch.mock.calls[1][0]).toBe(grant.url);
    expect(fetch.mock.calls[1][1]).toMatchObject({
      body: file, headers: grant.headers, credentials: "omit", redirect: "error", referrerPolicy: "no-referrer",
    });
    expect(fetch.mock.calls[2][0]).toBe("/api/uploads/upload-id/complete");
    expect(JSON.parse(fetch.mock.calls[2][1].body)).toEqual(input);
    installed(json({ path: "/workspace/.sulion-paste/file", size: 3 }));
    await expect(upload).resolves.toEqual({ path: "/workspace/.sulion-paste/file", size: 3 });
  });

  it("does not fall back to multipart or automatically retry a rejected public upload", async () => {
    window.__APP_CONFIG__ = { publicUploadOrigin: window.location.origin };
    const fetch = vi.fn().mockResolvedValue(json({ id: "upload-id", grant }))
      .mockResolvedValueOnce(new Response("Forbidden", { status: 403, statusText: "Forbidden" }));
    vi.stubGlobal("fetch", fetch);
    await expect(uploadFile(target, "", new File(["abc"], "file"))).rejects.toThrow("Forbidden");
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("does not complete a failed PUT and starts fresh only when the user retries", async () => {
    window.__APP_CONFIG__ = { publicUploadOrigin: window.location.origin };
    const fetch = vi.fn().mockResolvedValueOnce(json({ id: "first", grant }))
      .mockRejectedValueOnce(new Error(grant.url))
      .mockResolvedValueOnce(json({ id: "second", grant }))
      .mockResolvedValueOnce(new Response(null))
      .mockResolvedValueOnce(json({ path: "/workspace/file", size: 3 }));
    vi.stubGlobal("fetch", fetch);
    const file = new File(["abc"], "file");
    await expect(uploadFile(target, "", file)).rejects.toThrow("File transfer was interrupted");
    expect(fetch).toHaveBeenCalledTimes(2);
    await expect(uploadFile(target, "", file)).resolves.toHaveProperty("path");
    expect(fetch.mock.calls[2][0]).toBe("/api/uploads");
    expect(fetch.mock.calls[4][0]).toBe("/api/uploads/second/complete");
  });

  it("reports lost completion without replaying the install", async () => {
    window.__APP_CONFIG__ = { publicUploadOrigin: window.location.origin };
    const fetch = vi.fn().mockResolvedValueOnce(json({ id: "id", grant }))
      .mockResolvedValueOnce(new Response(null)).mockRejectedValueOnce(new TypeError("network error"));
    vi.stubGlobal("fetch", fetch);
    await expect(uploadFile(target, "", new File(["abc"], "file"))).rejects.toThrow("Check the destination");
    expect(fetch).toHaveBeenCalledTimes(3);
  });

  it("redacts storage network errors and rejects non-success statuses", async () => {
    const fetch = vi.fn().mockRejectedValueOnce(new Error(grant.url))
      .mockResolvedValueOnce(new Response(null, { status: 412 }));
    vi.stubGlobal("fetch", fetch);
    const file = new File([], "file");
    await expect(putStagedFile(grant, file)).rejects.toThrow("File transfer was interrupted");
    await expect(putStagedFile(grant, file)).rejects.toThrow("HTTP 412");
  });

  it("recognizes only the explicit LFI code, leaving other 403s distinct", async () => {
    const input = { repo: "repo", directory: "", filename: "file", size: 0, checksum: "checksum" };
    vi.stubGlobal("fetch", vi.fn().mockResolvedValueOnce(new Response(JSON.stringify({ code: "WAF_LFI_BODY" }), { status: 403 }))
      .mockResolvedValueOnce(new Response("Forbidden", { status: 403, statusText: "Forbidden" })));
    await expect(createUpload(input)).rejects.toThrow("Security policy blocked this request (LFI_BODY)");
    await expect(createUpload(input)).rejects.toThrow("Forbidden");
  });
});
