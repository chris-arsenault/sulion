import { expect, test } from "@playwright/test";

import { gotoApp } from "./helpers";

test("opens sessions directly into the timeline-only mobile workspace", async ({
  page,
}) => {
  await page.addInitScript(() => {
    localStorage.setItem("sulion.display.mode.v1", "terminal");
  });
  await gotoApp(page);

  const sessionsDrawer = page.getByRole("complementary", { name: "Sessions" });
  await expect(page.getByRole("button", { name: "Open sessions drawer" })).toBeVisible();
  await page.getByRole("button", { name: "Open sessions drawer" }).click();
  await expect(sessionsDrawer).toBeVisible();

  await page.locator('[data-session-name="Atlas Claude"]').click();
  await expect(sessionsDrawer).toBeHidden();
  await expect(page.getByRole("tab", { name: /Atlas Claude · time/i })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await expect(page.getByRole("tab", { name: /Atlas Claude · term/i })).toHaveCount(0);
  await expect(page.getByTestId("terminal-pane")).toHaveCount(0);
  await expect(page.getByTestId("timeline-pane")).toBeVisible();
  await expect(page.getByLabel("Agent prompt controls")).toBeVisible();
  const firstTurn = page.getByTestId("timeline-pane").getByTestId("turn-row").first();
  await expect(firstTurn).toBeVisible();
  expect(
    await page.locator(".wa--single > .wa__pane").evaluate(
      (pane) => pane.getBoundingClientRect().height,
    ),
  ).toBeGreaterThan(200);

  await firstTurn.click();
  await expect(page.getByRole("button", { name: "Back to timeline" })).toBeVisible();
  await expect(page.getByTestId("inspector-pane")).toBeVisible();
  await expect(page.getByTestId("inspector-overlay")).toHaveCount(0);
  await expect(page.getByLabel("Agent prompt controls")).toBeVisible();

  const toolRows = page.getByTestId("tool-pair-row");
  expect(await toolRows.count()).toBeGreaterThan(0);
  await expect(toolRows.locator(".td__tool-body")).toHaveCount(0);
  const readTool = page.locator(
    '[data-testid="tool-pair-row"][data-tool-type="read"]',
  );
  const firstTool = toolRows.first();
  await (await readTool.count() > 0 ? readTool.first() : firstTool).hover();
  await page.waitForTimeout(600);
  await expect(page.getByTestId("tool-hover-card")).toHaveCount(0);

  await firstTool.getByRole("button", { name: "Expand tool details" }).click();
  await expect(firstTool.locator(".td__tool-body")).toBeVisible();

  expect(
    await page.evaluate(() => {
      const detail = document.querySelector(".timeline-pane__mobile-detail")?.getBoundingClientRect();
      const prompt = document.querySelector(".timeline-prompt")?.getBoundingClientRect();
      return {
        detailHeight: detail?.height ?? 0,
        detailAbovePrompt: Boolean(detail && prompt && detail.bottom <= prompt.top + 1),
        promptFitsViewport: (prompt?.bottom ?? Number.POSITIVE_INFINITY) <= window.innerHeight,
      };
    }),
  ).toMatchObject({
    detailAbovePrompt: true,
    promptFitsViewport: true,
  });

  await page.getByRole("button", { name: "Back to timeline" }).click();
  await expect(page.getByTestId("turn-row").first()).toBeVisible();

  await page.keyboard.press("Control+Shift+D");
  await page.keyboard.press("Control+Shift+E");
  await expect(page.getByRole("dialog", { name: /terminal/i })).toHaveCount(0);
  expect(
    await page.evaluate(() => localStorage.getItem("sulion.display.mode.v1")),
  ).toBe("terminal");

  expect(
    await page.evaluate(() => ({
      noHorizontalOverflow: document.body.scrollWidth <= window.innerWidth,
      layoutFitsViewport:
        (document.querySelector(".layout")?.getBoundingClientRect().bottom ??
          Number.POSITIVE_INFINITY) <= window.innerHeight,
      promptFitsViewport:
        (document.querySelector(".timeline-prompt")?.getBoundingClientRect()
          .bottom ?? Number.POSITIVE_INFINITY) <= window.innerHeight,
    })),
  ).toMatchObject({
    noHorizontalOverflow: true,
    layoutFitsViewport: true,
    promptFitsViewport: true,
  });
});
