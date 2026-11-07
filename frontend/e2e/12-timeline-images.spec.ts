import { expect, test } from "@playwright/test";

import {
  createLabeledSession,
  deleteSessionById,
  expectTerminalToContain,
  gotoApp,
  openSession,
  runTerminalCommand,
} from "./helpers";

const DIRECTORY = ".artifacts/harness";
const FILES = [
  "raised5-plates.png",
  "raised5-heights.png",
  "raised4-heights.png",
];

test("renders Claude's local image references inline and opens the matching files on desktop and mobile", async ({
  page,
  request,
}, testInfo) => {
  await page.setContent(
    '<div style="width:640px;height:240px;background:linear-gradient(120deg,#276149,#aacbbb,#739ec7);display:grid;place-items:center;font:24px monospace">Local image preview fixture</div>',
  );
  const png = await page.locator("div").screenshot();
  for (const name of FILES) {
    const response = await request.post(
      `/api/repos/atlas/upload?path=${encodeURIComponent(DIRECTORY)}`,
      {
        multipart: { file: { name, mimeType: "image/png", buffer: png } },
      },
    );
    expect(response.ok(), await response.text()).toBeTruthy();
  }
  const label = "Claude image reference review";
  const sessionId = await createLabeledSession(request, "atlas", label);
  try {
    await gotoApp(page);
    await openSession(page, label);
    await runTerminalCommand(page, "sulion-agent --type claude --mode mock --");
    await expectTerminalToContain(page, "Type a prompt and press Enter.");
    const names = FILES.map((name) => `\`${name}\``).join(", ");
    const prompt = `Pictures are in \`/home/sulion/repos/atlas/${DIRECTORY}/\`: ${names}.`;
    await runTerminalCommand(page, prompt);
    const turn = page
      .getByTestId("turn-row")
      .filter({ hasText: "Pictures are in" })
      .first();
    await expect(turn).toBeVisible({ timeout: 20_000 });
    await turn.click();
    const output = page
      .getByTestId("turn-detail")
      .locator(".td__sub--assistant")
      .first();
    await expect(output.locator("img.md__image")).toHaveCount(3);
    for (const name of FILES) {
      const image = output.getByRole("img", { name, exact: true });
      await image.scrollIntoViewIfNeeded();
      await expect(image).toHaveAttribute("src", /^blob:/);
      await expect
        .poll(() =>
          image.evaluate(
            (element) => (element as HTMLImageElement).naturalWidth,
          ),
        )
        .toBe(640);
      await expect(image.locator("..")).toHaveAttribute(
        "href",
        `${DIRECTORY}/${name}`,
      );
    }
    await page.screenshot({
      path: testInfo.outputPath("desktop-inline-images.png"),
    });
    await page.setViewportSize({ width: 390, height: 844 });
    const mobileOutput = page
      .getByTestId("turn-detail")
      .locator(".td__sub--assistant")
      .first();
    const image = mobileOutput.getByRole("img", {
      name: FILES[0]!,
      exact: true,
    });
    await image.scrollIntoViewIfNeeded();
    await expect
      .poll(() =>
        image.evaluate((element) => (element as HTMLImageElement).naturalWidth),
      )
      .toBe(640);
    expect(
      await mobileOutput.evaluate(
        (element) => element.scrollWidth <= element.clientWidth,
      ),
    ).toBeTruthy();
    await page.screenshot({
      path: testInfo.outputPath("mobile-inline-images.png"),
    });
    await image.click();
    await expect(
      page.getByTestId("file-tab").locator(".ft__img"),
    ).toBeVisible();
    await expect(page.getByTestId("file-tab")).toContainText(
      `${DIRECTORY}/${FILES[0]}`,
    );
    await expect(page.getByRole("dialog", { name: "Turn output" })).toHaveCount(
      0,
    );
  } finally {
    await deleteSessionById(request, sessionId);
  }
});
