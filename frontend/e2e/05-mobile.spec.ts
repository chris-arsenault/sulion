import { expect, test } from "@playwright/test";

import { deleteSessionById, gotoApp } from "./helpers";

test("uses compact mobile navigation and starts sessions through the repository picker", async ({
  page,
  request,
}, testInfo) => {
  await page.addInitScript(() => {
    if (!localStorage.getItem("sulion.sidebar.view.desktop.v1"))
      localStorage.setItem("sulion.sidebar.view.desktop.v1", "full");
  });
  await gotoApp(page);
  await page.getByRole("button", { name: "Open sessions drawer" }).click();
  const drawer = page.getByRole("complementary", { name: "Sessions" });
  const views = drawer.getByRole("group", { name: "Sidebar view" });
  await expect(
    views.getByRole("button", { name: "Open", exact: true }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(drawer.locator("[data-repo-name]")).toHaveCount(0);
  await expect
    .poll(() => drawer.evaluate((element) => element.getBoundingClientRect().x))
    .toBe(0);
  await page.screenshot({ path: testInfo.outputPath("mobile-open.png") });
  await drawer
    .getByRole("searchbox", { name: "Search sessions" })
    .fill("Atlas Claude");
  await expect(drawer.locator("[data-session-name]")).toHaveCount(1);
  await drawer
    .getByRole("button", { name: "Actions for Atlas Claude" })
    .click();
  await expect(
    page.getByRole("menuitem", { name: "Open timeline", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("menuitem", { name: "Open terminal", exact: true }),
  ).toHaveCount(0);
  await page
    .getByRole("button", { name: "Dismiss context menu" })
    .click({ position: { x: 2, y: 2 } });
  expect(
    await drawer
      .getByRole("button", { name: "Actions for Atlas Claude" })
      .evaluate((button) => button.getBoundingClientRect().height),
  ).toBeGreaterThanOrEqual(44);
  expect(
    await drawer.evaluate(
      (element) => element.getBoundingClientRect().width / window.innerWidth,
    ),
  ).toBeGreaterThan(0.9);
  await drawer
    .getByRole("button", { name: "New session", exact: true })
    .click();
  const dialog = page.getByRole("dialog", { name: "New session" });
  await dialog
    .getByRole("searchbox", { name: "Search repositories" })
    .fill("zephyr");
  await dialog.getByRole("button", { name: "zephyr Repository" }).click();
  await dialog.getByLabel("workspace mode").selectOption("main");
  await page.screenshot({
    path: testInfo.outputPath("mobile-new-session.png"),
  });
  const createdResponse = page.waitForResponse(
    (response) =>
      response.url().endsWith("/api/sessions") &&
      response.request().method() === "POST",
  );
  await dialog.getByRole("button", { name: "Spawn" }).click();
  const response = await createdResponse;
  expect(response.ok()).toBeTruthy();
  const { id } = (await response.json()) as { id: string };
  try {
    await expect(drawer).toBeHidden();
    await expect(page.getByRole("tab", { name: /· time/i })).toBeVisible();
    await expect(page.getByRole("tab", { name: /· term/i })).toHaveCount(0);
    await page.getByRole("button", { name: "Open sessions drawer" }).click();
    await views.getByRole("button", { name: "Sessions", exact: true }).click();
    await page.reload();
    await page.getByRole("button", { name: "Open sessions drawer" }).click();
    await expect(
      views.getByRole("button", { name: "Sessions", exact: true }),
    ).toHaveAttribute("aria-pressed", "true");
    await page.setViewportSize({ width: 1440, height: 1024 });
    await expect(
      page
        .getByRole("group", { name: "Sidebar view" })
        .getByRole("button", { name: "Full", exact: true }),
    ).toHaveAttribute("aria-pressed", "true");
  } finally {
    await deleteSessionById(request, id);
  }
});

test("opens sessions directly into the timeline-only mobile workspace", async ({
  page,
}) => {
  await page.addInitScript(() => {
    localStorage.setItem("sulion.display.mode.v1", "terminal");
  });
  await gotoApp(page);

  const sessionsDrawer = page.getByRole("complementary", { name: "Sessions" });
  await expect(
    page.getByRole("button", { name: "Open sessions drawer" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Open sessions drawer" }).click();
  await expect(sessionsDrawer).toBeVisible();

  await page.locator('[data-session-name="Atlas Claude"]').click();
  await expect(sessionsDrawer).toBeHidden();
  await expect(
    page.getByRole("tab", { name: /Atlas Claude · time/i }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(
    page.getByRole("tab", { name: /Atlas Claude · term/i }),
  ).toHaveCount(0);
  await expect(page.getByTestId("terminal-pane")).toHaveCount(0);
  await expect(page.getByTestId("timeline-pane")).toBeVisible();
  await expect(page.getByLabel("Agent prompt controls")).toBeVisible();
  const firstTurn = page
    .getByTestId("timeline-pane")
    .getByTestId("turn-row")
    .first();
  await expect(firstTurn).toBeVisible();
  expect(
    await page
      .locator(".wa--single > .wa__pane")
      .evaluate((pane) => pane.getBoundingClientRect().height),
  ).toBeGreaterThan(200);

  await firstTurn.click();
  await expect(
    page.getByRole("button", { name: "Back to timeline" }),
  ).toBeVisible();
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
  await ((await readTool.count()) > 0 ? readTool.first() : firstTool).hover();
  await page.waitForTimeout(600);
  await expect(page.getByTestId("tool-hover-card")).toHaveCount(0);

  await firstTool.getByRole("button", { name: "Expand tool details" }).click();
  await expect(firstTool.locator(".td__tool-body")).toBeVisible();

  expect(
    await page.evaluate(() => {
      const detail = document
        .querySelector(".timeline-pane__mobile-detail")
        ?.getBoundingClientRect();
      const prompt = document
        .querySelector(".timeline-prompt")
        ?.getBoundingClientRect();
      return {
        detailHeight: detail?.height ?? 0,
        detailAbovePrompt: Boolean(
          detail && prompt && detail.bottom <= prompt.top + 1,
        ),
        promptFitsViewport:
          (prompt?.bottom ?? Number.POSITIVE_INFINITY) <= window.innerHeight,
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
