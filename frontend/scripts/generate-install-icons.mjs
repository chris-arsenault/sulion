/* global document -- page.evaluate runs inside Chromium */
import { mkdir } from "node:fs/promises";
import { fileURLToPath, URL } from "node:url";
import { chromium } from "@playwright/test";

const source = new URL("../public/favicon.svg", import.meta.url);
const output = new URL("../public/icons/", import.meta.url);
await mkdir(output, { recursive: true });

const browser = await chromium.launch();
try {
  for (const [filename, size] of [
    ["icon-192.png", 192],
    ["icon-512.png", 512],
    ["apple-touch-icon.png", 180],
  ]) {
    const page = await browser.newPage({
      viewport: { width: size, height: size },
      deviceScaleFactor: 1,
    });
    await page.goto(source.href);
    await page.evaluate(() => {
      const background = document.querySelector("rect").getAttribute("fill");
      document.documentElement.style.backgroundColor = background;
    });
    await page.screenshot({ path: fileURLToPath(new URL(filename, output)) });
    await page.close();
  }
} finally {
  await browser.close();
}
