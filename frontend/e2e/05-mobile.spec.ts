import { expect, test } from "@playwright/test";

import { gotoApp } from "./helpers";

test("opens the sessions drawer and lets mobile users switch between session tabs", async ({
  page,
}) => {
  await gotoApp(page);

  const sessionsDrawer = page.getByRole("complementary", { name: "Sessions" });
  await expect(page.getByRole("button", { name: "Open sessions drawer" })).toBeVisible();
  await page.getByRole("button", { name: "Open sessions drawer" }).click();
  await expect(sessionsDrawer).toBeVisible();

  await page.locator('[data-session-name="Atlas Claude"]').click();
  await expect(sessionsDrawer).toBeHidden();
  await expect(page.getByRole("tab", { name: /Atlas Claude · time/i })).toBeVisible();
  await expect(page.getByRole("tab", { name: /Atlas Claude · term/i })).toBeVisible();

  await page.getByRole("tab", { name: /Atlas Claude · time/i }).click();
  await expect(page.getByTestId("timeline-pane")).toBeVisible();
});
