import fs from "node:fs";
import path from "node:path";

import { expect, test, type Locator, type Page } from "@playwright/test";

import {
  gotoApp,
  openContextMenu,
  openSession,
  openTreeFile,
  repoGroup,
  selectMenuItem,
  treeRow,
  visibleTimelinePane,
} from "./helpers";

/**
 * Screenshot tour — walks the major surfaces and captures PNGs that feed
 * `docs/user-guide.md`. Gated behind SULION_SCREENSHOT_TOUR so it does not
 * fire during a normal `make e2e`.
 *
 *   SULION_SCREENSHOT_TOUR=1 make e2e
 *   python3 scripts/crop_screenshots.py
 *
 * The tour writes full-viewport PNGs plus a `crops.json` manifest of
 * bounding boxes under `docs/screenshots/raw/`. The cropper turns those
 * into the feature-focused PNGs the guide references.
 *
 * Every step swallows its own failure so one drifted selector costs one
 * crop, not the whole tour. The desktop test and the mobile test share the
 * manifest; each rewrites it with everything recorded so far.
 */

const SCREENSHOT_DIR = path.resolve(
  import.meta.dirname,
  "..",
  "..",
  "docs",
  "screenshots",
  "raw",
);

const TOUR_PLAN_TITLE = "User guide screenshots";
const ATLAS_CLAUDE_ROW = '[data-session-name="Atlas Claude"]';

interface CropEntry {
  name: string;
  source: string;
  x: number;
  y: number;
  width: number;
  height: number;
  pad: number;
}

const crops: CropEntry[] = [];

test.skip(
  !process.env.SULION_SCREENSHOT_TOUR,
  "screenshot tour runs on demand — set SULION_SCREENSHOT_TOUR=1",
);

test.describe.configure({ mode: "serial" });

test.describe("desktop", () => {
  test.use({ viewport: { width: 1440, height: 1024 } });

  test("screenshot tour", async ({ page }) => {
    test.setTimeout(360_000);
    fs.mkdirSync(SCREENSHOT_DIR, { recursive: true });

    await gotoApp(page);
    const sidebar = page.locator(".sidebar");
    const sidebarViews = page.getByRole("group", { name: "Sidebar view" });

    await step("01-overview", async () => {
      await sidebarViews.getByRole("button", { name: "Full", exact: true }).click();
      await openSession(page, "Atlas Claude");
      await visibleTimelinePane(page).getByTestId("turn-row").first().waitFor();
      await settle(page);
      await shot(page, "01-overview.png");
      await recordCrop("01-sidebar", "01-overview.png", sidebar, 4);
      await recordCrop("01-workspace", "01-overview.png", page.locator(".wa").first(), 4);
    });

    await step("13-sidebar-views", async () => {
      await sidebarViews.getByRole("button", { name: "Sessions", exact: true }).click();
      await expect(page.locator(ATLAS_CLAUDE_ROW)).toBeVisible();
      await settle(page);
      await shot(page, "13-sidebar-sessions.png");
      await recordCrop("13-sidebar-views", "13-sidebar-sessions.png", sidebar, 4);

      await sidebarViews.getByRole("button", { name: "Open", exact: true }).click();
      await expect(page.locator(ATLAS_CLAUDE_ROW)).toBeVisible();
      await settle(page);
      await shot(page, "14-sidebar-open.png");
      await recordCrop("14-sidebar-open", "14-sidebar-open.png", sidebar, 4);

      await sidebarViews.getByRole("button", { name: "Full", exact: true }).click();
      await expect(page.getByRole("button", { name: /^Files/ }).first()).toBeVisible();
    });

    await step("02-command-palette", async () => {
      await page.getByRole("button", { name: "Open command palette" }).click();
      const input = page.getByPlaceholder("Type a command or jump to…");
      await input.fill("Atlas");
      await expect(
        page.getByRole("option", { name: /Open session · atlas \/ Atlas Codex/i }),
      ).toBeVisible();
      await settle(page);
      await shot(page, "02-command-palette.png");
      await recordCrop(
        "02-command-palette",
        "02-command-palette.png",
        page.getByRole("dialog").first(),
        12,
      );
      await page.keyboard.press("Escape");
    });

    await step("03-timeline-turn", async () => {
      const timeline = visibleTimelinePane(page);
      const firstTurn = timeline.getByTestId("turn-row").first();
      await firstTurn.click();
      await expect(page.getByLabel("Prompt actions")).toBeVisible();
      await settle(page);
      await shot(page, "03-timeline-turn.png");
      await recordCrop("03-timeline-turn", "03-timeline-turn.png", timeline, 4);
    });

    await step("04-thinking-flyout", async () => {
      await page.getByLabel("View thinking").first().click();
      const flyout = page.getByTestId("thinking-flyout");
      await expect(flyout).toBeVisible();
      await settle(page);
      await shot(page, "04-thinking-flyout.png");
      await recordCrop("04-thinking-flyout", "04-thinking-flyout.png", flyout, 12);
      await page.getByLabel("Close thinking").click();
    });

    await step("05-tool-hover", async () => {
      const readTool = page
        .locator('[data-testid="tool-pair-row"][data-tool-type="read"]')
        .first();
      await readTool.hover();
      const card = page.getByTestId("tool-hover-card");
      await expect(card).toBeVisible();
      await shot(page, "05-tool-hover.png");
      await recordCrop("05-tool-hover", "05-tool-hover.png", card, 16);
      await settle(page);
    });

    await step("18-timeline-settings", async () => {
      await page.getByRole("button", { name: "Timeline settings" }).click();
      const flyout = page.getByTestId("timeline-controls-flyout");
      await expect(flyout).toBeVisible();
      await settle(page);
      await shot(page, "18-timeline-settings.png");
      await recordCrop("18-timeline-settings", "18-timeline-settings.png", flyout, 12);
      await page.getByRole("button", { name: "Close timeline settings" }).click();
    });

    await step("19-submitted-prompts", async () => {
      await page.getByTestId("submitted-prompts-button").first().click();
      const dialog = page
        .getByRole("dialog")
        .filter({ hasText: "Submitted Prompts" })
        .first();
      await expect(dialog).toBeVisible();
      await settle(page);
      await shot(page, "19-submitted-prompts.png");
      await recordCrop("19-submitted-prompts", "19-submitted-prompts.png", dialog, 12);
      await page.keyboard.press("Escape");
      await expect(dialog).toBeHidden();
    });

    await step("06-file-tab", async () => {
      await openTreeFile(page, "atlas", "src/lib.rs");
      const fileTab = page.locator('[data-testid="file-tab"]:visible').first();
      await expect(fileTab).toBeVisible();
      // The trace section renders collapsed; expand it so the rows show.
      const trace = fileTab.getByLabel("Related timeline turns");
      if ((await trace.count()) > 0) {
        const toggle = trace.getByRole("button", { name: "Related timeline turns" });
        if (
          (await toggle.count()) > 0 &&
          (await toggle.getAttribute("aria-expanded")) !== "true"
        ) {
          await toggle.click();
        }
        await expect(trace.locator(".ft__trace-button").first()).toBeVisible();
      }
      await settle(page);
      await shot(page, "06-file-tab.png");
      await recordCrop("06-file-tab", "06-file-tab.png", fileTab, 6);
      if ((await trace.count()) > 0) {
        await recordCrop("06-file-trace", "06-file-tab.png", trace.first(), 10);
      }
    });

    await step("07-diff-tab", async () => {
      await openContextMenu(treeRow(page, "atlas", "src/lib.rs"));
      await selectMenuItem(page, "Open diff");
      const diffTab = page.locator('[data-testid="diff-tab"]:visible').first();
      await expect(diffTab).toBeVisible();
      await settle(page);
      await shot(page, "07-diff-tab.png");
      await recordCrop("07-diff-tab", "07-diff-tab.png", diffTab, 6);
    });

    await step("08-context-menu", async () => {
      await openContextMenu(page.locator(ATLAS_CLAUDE_ROW));
      const menu = page.getByRole("menu").first();
      await expect(menu).toBeVisible();
      await shot(page, "08-context-menu.png");
      await recordCrop("08-context-menu", "08-context-menu.png", menu, 12);
      await page.keyboard.press("Escape");
    });

    await step("09-session-pinned", async () => {
      const row = page.locator(ATLAS_CLAUDE_ROW);
      await openContextMenu(row);
      await selectMenuItem(page, "Pin to top");
      await openContextMenu(row);
      await page.getByRole("menuitem", { name: "Colour" }).hover();
      await page.getByRole("menuitem", { name: "teal" }).click();
      await expect(row.getByLabel("pinned")).toBeVisible();
      await settle(page);
      await shot(page, "09-session-pinned.png");
      await recordCrop("09-session-pinned", "09-session-pinned.png", sidebar, 4);
      await openContextMenu(row);
      await selectMenuItem(page, "Unpin");
      await openContextMenu(row);
      await page.getByRole("menuitem", { name: "Colour" }).hover();
      await page.getByRole("menuitem", { name: "None" }).click();
    });

    await step("10-stats-strip", async () => {
      const stats = page.getByTestId("stats-strip");
      await expect(stats).toBeVisible();
      const toggle = page.getByRole("button", { name: "Toggle stats details" });
      if ((await toggle.count()) > 0) {
        await toggle.first().click();
      }
      await settle(page);
      await shot(page, "10-stats-strip.png");
      await recordCrop("10-stats-strip", "10-stats-strip.png", stats, 8);
      if ((await toggle.count()) > 0) {
        await toggle.first().click();
      }
    });

    await step("11-codex-subagent", async () => {
      await openSession(page, "Atlas Codex");
      const timeline = visibleTimelinePane(page);
      await timeline.getByTestId("turn-row").first().click();
      await expect(
        page.locator('[data-testid="tool-pair-row"][data-tool-type="task"]').first(),
      ).toBeVisible();
      await page.getByRole("button", { name: /View agent log/i }).first().click();
      const modal = page.getByTestId("subagent-modal");
      await expect(modal).toBeVisible();
      await settle(page);
      await shot(page, "11-codex-subagent.png");
      await recordCrop("11-codex-subagent", "11-codex-subagent.png", modal, 16);
      await page.keyboard.press("Escape");
      await expect(modal).toBeHidden();
    });

    await step("12-repo-timeline", async () => {
      await openRepoMenu(page, "atlas");
      await selectMenuItem(page, "Open repo timeline");
      const timeline = visibleTimelinePane(page);
      const turn = timeline.getByTestId("turn-row").first();
      const empty = timeline.locator(".timeline-pane__empty");
      await expect(turn.or(empty).first()).toBeVisible();
      if ((await turn.count()) === 0) {
        // Empty repo timeline is a loading or data gap, not a surface worth
        // documenting; leave the old crop out rather than ship a blank pane.
        console.warn(
          `12-repo-timeline: repo timeline is empty (${(await empty.textContent())?.trim()}), crop skipped`,
        );
        return;
      }
      await settle(page);
      await shot(page, "12-repo-timeline.png");
      await recordCrop("12-repo-timeline", "12-repo-timeline.png", timeline, 4);
    });

    await step("20-plans", async () => {
      await openRepoMenu(page, "atlas");
      await selectMenuItem(page, "Open published plans");
      const dialog = page.locator(".plan-modal").getByRole("dialog").first();
      await expect(dialog).toBeVisible();
      await expect(dialog.getByText("Loading plans…")).toHaveCount(0);

      const existing = dialog.getByText(TOUR_PLAN_TITLE, { exact: true });
      if ((await existing.count()) > 0) {
        await existing.first().click();
      } else {
        const newPlan = dialog.getByRole("button", { name: "New plan" });
        if ((await newPlan.getAttribute("aria-expanded")) !== "true") {
          await newPlan.click();
        }
        await dialog.getByLabel("Title", { exact: true }).fill(TOUR_PLAN_TITLE);
        await dialog
          .getByLabel("Short description")
          .fill("Regenerate the user-guide screenshots against the e2e stack.");
        await dialog
          .getByLabel("Phases", { exact: true })
          .fill(
            [
              "Capture surfaces | Tour every major view with Playwright",
              "Crop and review | Crop to feature regions and check each PNG",
            ].join("\n"),
          );
        await dialog.getByRole("button", { name: "Start plan" }).click();
      }
      await expect(dialog.getByLabel("Back to plans")).toBeVisible();
      await expect(dialog.getByText("Capture surfaces")).toBeVisible();
      await settle(page);
      await shot(page, "20-plans.png");
      await recordCrop("20-plans", "20-plans.png", dialog, 12);

      await dialog.getByLabel("Back to plans").click();
      await expect(dialog.getByText(TOUR_PLAN_TITLE)).toBeVisible();
      await settle(page);
      await shot(page, "20-plan-index.png");
      await recordCrop("20-plan-index", "20-plan-index.png", dialog, 12);
      await page.keyboard.press("Escape");
      await expect(dialog).toBeHidden();
    });

    await step("15-overview", async () => {
      // The rail button opens Overview as a tab; Ctrl/Cmd-M opens the modal
      // whose header links swap to Metrics and Display in place.
      await page.keyboard.press("Control+m");
      const dialog = page.getByRole("dialog", { name: "Team overview" });
      await expect(dialog).toBeVisible();
      const firstTeam = dialog.locator(".monitor-team").first();
      await expect(firstTeam).toBeVisible();
      await settle(page);
      await shot(page, "15-overview.png");
      // The modal is viewport-wide; with one seeded team most of it is
      // empty, so crop to the header bar plus the team section.
      await recordCropUnion(
        "15-overview",
        "15-overview.png",
        [dialog.locator(".monitor-pane__bar"), firstTeam],
        8,
      );

      await dialog.getByRole("button", { name: "metrics", exact: true }).click();
      const metrics = page.getByTestId("metrics-pane");
      await expect(metrics).toBeVisible();
      const flow = metrics.getByRole("region", { name: "Delivery flow" });
      await expect(flow).toBeVisible();
      await settle(page);
      await shot(page, "16-metrics.png");
      await recordCrop("16-metrics", "16-metrics.png", dialog, 8);
      await flow.scrollIntoViewIfNeeded();
      await settle(page);
      await shot(page, "16-metrics-flow.png");
      await recordCrop("16-metrics-flow", "16-metrics-flow.png", flow, 10);

      await dialog.getByRole("button", { name: "display", exact: true }).click();
      const display = page.getByTestId("display-settings");
      await expect(display).toBeVisible();
      await settle(page);
      await shot(page, "17-display-settings.png");
      await recordCrop("17-display-settings", "17-display-settings.png", display, 12);
      await page.keyboard.press("Escape");
      await expect(dialog).toBeHidden();
    });

    await step("21-secrets", async () => {
      await page.getByRole("button", { name: "Open secrets manager" }).click();
      const secrets = page.locator(".secrets-tab:visible").first();
      await expect(secrets).toBeVisible();
      await expect(page.getByRole("heading", { name: "New bundle" })).toBeVisible();
      await settle(page);
      await shot(page, "21-secrets.png");
      await recordCrop("21-secrets", "21-secrets.png", secrets, 6);
    });

    await step("22-new-session", async () => {
      // The searchable repo/meta-repo picker hangs off the compact views'
      // "New session" button; Full view's per-repo "+" starts a session
      // directly in that repo.
      await sidebarViews.getByRole("button", { name: "Sessions", exact: true }).click();
      await sidebar.getByRole("button", { name: "New session", exact: true }).click();
      const dialog = page.getByRole("dialog", { name: "New session" });
      await expect(dialog).toBeVisible();
      await expect(dialog.getByRole("button", { name: "atlas Repository" })).toBeVisible();
      await settle(page);
      await shot(page, "22-new-session.png");
      await recordCrop("22-new-session", "22-new-session.png", dialog, 12);
      await page.keyboard.press("Escape");
      await expect(dialog).toBeHidden();
      await sidebarViews.getByRole("button", { name: "Full", exact: true }).click();
    });

    writeManifest();
  });
});

test.describe("mobile", () => {
  test.use({
    viewport: { width: 390, height: 844 },
    isMobile: true,
    hasTouch: true,
  });

  test("mobile screenshot tour", async ({ page }) => {
    test.setTimeout(120_000);
    fs.mkdirSync(SCREENSHOT_DIR, { recursive: true });

    await gotoApp(page);
    const drawerButton = page.getByRole("button", { name: "Open sessions drawer" });
    const drawer = page.getByRole("complementary", { name: "Sessions" });
    const viewport = page.locator("body");

    await step("23-mobile-timeline", async () => {
      await drawerButton.click();
      await expect(drawer).toBeVisible();
      await page.locator(ATLAS_CLAUDE_ROW).click();
      await expect(drawer).toBeHidden();
      await expect(
        page.getByTestId("timeline-pane").getByTestId("turn-row").first(),
      ).toBeVisible();
      await expect(page.getByLabel("Agent prompt controls")).toBeVisible();
      await shot(page, "23-mobile-timeline.png");
      await recordCrop("23-mobile-timeline", "23-mobile-timeline.png", viewport, 0);
    });

    await step("24-mobile-drawer", async () => {
      await drawerButton.click();
      await expect(drawer).toBeVisible();
      await expect
        .poll(() => drawer.evaluate((element) => element.getBoundingClientRect().x))
        .toBe(0);
      await shot(page, "24-mobile-drawer.png");
      await recordCrop("24-mobile-drawer", "24-mobile-drawer.png", viewport, 0);
    });

    writeManifest();
  });
});

async function openRepoMenu(page: Page, repo: string): Promise<void> {
  const group = repoGroup(page, repo);
  await expect(group).toBeVisible();
  await openContextMenu(group.locator(".sidebar__group-toggle").first());
  await expect(page.getByRole("menu").first()).toBeVisible();
}

async function step(name: string, body: () => Promise<void>): Promise<void> {
  await test.step(name, async () => {
    try {
      await body();
    } catch (error) {
      console.warn(`step ${name} failed:`, error);
    }
  });
}

/** Park the pointer over the terminal body so hover tooltips do not leak
 * into the next shot. */
async function settle(page: Page): Promise<void> {
  const viewport = page.viewportSize();
  if (viewport && viewport.width >= 1000) {
    // Right edge: terminal pane in the workspace, scrim under a modal —
    // neither carries a tooltip or hover card.
    await page.mouse.move(viewport.width - 6, 300);
  }
  await page.waitForTimeout(250);
}

async function shot(page: Page, name: string): Promise<void> {
  await page.waitForTimeout(250);
  await page.screenshot({
    path: path.join(SCREENSHOT_DIR, name),
    fullPage: false,
  });
}

function writeManifest(): void {
  fs.writeFileSync(
    path.join(SCREENSHOT_DIR, "crops.json"),
    JSON.stringify(crops, null, 2) + "\n",
  );
}

async function recordCrop(
  name: string,
  source: string,
  locator: Locator,
  pad: number,
): Promise<void> {
  await recordCropUnion(name, source, [locator], pad);
}

/** Record the bounding box that encloses every given element. */
async function recordCropUnion(
  name: string,
  source: string,
  locators: Locator[],
  pad: number,
): Promise<void> {
  try {
    let left = Number.POSITIVE_INFINITY;
    let top = Number.POSITIVE_INFINITY;
    let right = 0;
    let bottom = 0;
    for (const locator of locators) {
      if ((await locator.count()) === 0) continue;
      const box = await locator.boundingBox({ timeout: 5_000 });
      if (!box) continue;
      left = Math.min(left, box.x);
      top = Math.min(top, box.y);
      right = Math.max(right, box.x + box.width);
      bottom = Math.max(bottom, box.y + box.height);
    }
    if (!Number.isFinite(left) || right <= left || bottom <= top) {
      console.warn(`recordCrop: no bounding box for ${name}`);
      return;
    }
    const x = Math.max(0, Math.round(left));
    const y = Math.max(0, Math.round(top));
    crops.push({
      name,
      source,
      x,
      y,
      width: Math.round(right) - x,
      height: Math.round(bottom) - y,
      pad,
    });
  } catch (error) {
    console.warn(`recordCrop ${name} failed:`, error);
  }
}
