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
