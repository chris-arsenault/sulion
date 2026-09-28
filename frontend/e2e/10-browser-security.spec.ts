import { expect, test } from "@playwright/test";
import { createServer } from "node:http";

import { gotoApp, openTreeFile } from "./helpers";

test("nginx enforces headers and denies retired and internal routes", async ({ request }) => {
  for (const path of ["/", "/config.js", "/api/repos", "/pair"]) {
    const response = await request.get(path);
    const headers = response.headers();
    expect(headers["content-security-policy"]).toContain("frame-ancestors 'none'");
    expect(headers["content-security-policy"]).toContain("script-src 'self' 'wasm-unsafe-eval'");
    expect(headers["x-content-type-options"]).toBe("nosniff");
    expect(headers["referrer-policy"]).toBe("no-referrer");
    expect(headers["x-frame-options"]).toBe("DENY");
    expect(headers["strict-transport-security"]).toBeUndefined();
  }
  const https = await request.get("/", { headers: { "X-Forwarded-Proto": "https" } });
  expect(https.headers()["strict-transport-security"]).toBe("max-age=31536000");
  for (const path of [
    "/pair", "/api/devices/pair", "/api/repos/atlas/ingest", "/api/repos/atlas/raw",
    "/retrieval", "/retrieval/search", "/broker/v1/use", "/broker/v1/auth/check",
    "/broker/v1/pty-credentials", "/broker/v1/pty-credentials/example", "/ws/nodes",
  ]) {
    for (const headers of [{}, { Authorization: "Bearer retired-device-token" }]) {
      expect((await request.get(path, { headers })).status()).toBe(404);
    }
  }
});

test("CSP rejects inline scripts and permits PUTs only to the derived upload origin", async ({ page }) => {
  await gotoApp(page);
  const directive = await page.evaluate(() => new Promise<string>((resolve) => {
    document.addEventListener("securitypolicyviolation", (event) => resolve(event.effectiveDirective), { once: true });
    const script = document.createElement("script");
    script.textContent = "document.body.dataset.inlineExecuted = 'yes'";
    document.body.append(script);
  }));
  expect(directive).toBe("script-src-elem");
  expect(await page.locator("body").getAttribute("data-inline-executed")).toBeNull();

  // Only the remote storage response is a fixture. Chromium still enforces
  // the nginx policy before allowing the request to reach this route.
  const storage = "https://sulion-e2e-uploads.s3.us-east-1.amazonaws.com/fixture";
  let uploaded = "";
  await page.route(storage, async (route) => {
    uploaded = route.request().postData() ?? "";
    await route.fulfill({ status: 200, headers: { "access-control-allow-origin": "*" } });
  });
  // eslint-disable-next-line local/no-direct-fetch -- exercise Chromium's CSP, not the application request wrapper
  expect(await page.evaluate(async (url) => (await window.fetch(url, { method: "PUT", body: "fixture" })).ok, storage)).toBe(true);
  expect(uploaded).toBe("fixture");
  let unrelatedRequested = false;
  await page.route("https://unrelated-bucket.s3.us-east-1.amazonaws.com/fixture", async (route) => {
    unrelatedRequested = true;
    await route.fulfill({ status: 200, headers: { "access-control-allow-origin": "*" } });
  });
  expect(await page.evaluate(async () => {
    try {
      // eslint-disable-next-line local/no-direct-fetch -- the rejected origin must reach Chromium's CSP check
      await window.fetch("https://unrelated-bucket.s3.us-east-1.amazonaws.com/fixture", { method: "PUT", body: "fixture" });
      return false;
    } catch {
      return true;
    }
  })).toBe(true);
  expect(unrelatedRequested).toBe(false);
});

test("browser refuses to embed Sulion", async ({ page, baseURL }) => {
  const server = createServer((_req, res) => {
    res.setHeader("Content-Type", "text/html");
    res.end(`<iframe src="${baseURL}/"></iframe>`);
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    const address = server.address();
    if (!address || typeof address === "string") throw new Error("Missing test listener");
    const refusal = page.waitForEvent("console", {
      predicate: (message) => message.text().includes("frame-ancestors"),
    });
    await page.goto(`http://127.0.0.1:${address.port}`);
    await refusal;
    await expect(page.frameLocator("iframe").locator("#root")).toHaveCount(0);
  } finally {
    await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  }
});

test("CSP preserves Markdown, math, source highlighting and SVG blob previews", async ({ page, request }) => {
  const files = [
    { name: "policy.md", mimeType: "text/markdown", content: "# Policy rendering\n\n$$x^2 + y^2 = z^2$$\n\n```rust\nfn main() {}\n```\n" },
    { name: "policy.svg", mimeType: "image/svg+xml", content: '<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><rect width="32" height="32" fill="green"/></svg>' },
  ];
  for (const file of files) {
    const response = await request.post("/api/repos/atlas/upload?path=security-check", {
      multipart: { file: { name: file.name, mimeType: file.mimeType, buffer: Buffer.from(file.content) } },
    });
    expect(response.ok(), await response.text()).toBe(true);
  }
  const violations: string[] = [];
  page.on("console", (message) => {
    if (/violates.*Content Security Policy/i.test(message.text())) violations.push(message.text());
  });
  await gotoApp(page);
  await openTreeFile(page, "atlas", "security-check/policy.md");
  await expect(page.getByRole("heading", { name: "Policy rendering" })).toBeVisible();
  await expect(page.locator(".katex")).toBeVisible();
  await openTreeFile(page, "atlas", "src/lib.rs");
  await expect(page.locator(".shiki:visible")).toBeVisible();
  await openTreeFile(page, "atlas", "security-check/policy.svg");
  const preview = page.locator(".ft__img:visible");
  await expect(preview).toHaveAttribute("src", /^blob:/);
  await expect.poll(() => preview.evaluate((img) => (img as HTMLImageElement).naturalWidth)).toBe(32);
  expect(violations).toEqual([]);
});
