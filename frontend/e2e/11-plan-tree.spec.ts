import {
  expect,
  test,
  type APIRequestContext,
  type Page,
} from "@playwright/test";

import { gotoApp } from "./helpers";

const LEAF_PHASE_TITLE = "Work at depth 8";

interface PlanFixture {
  id: string;
  title: string;
}

async function createHierarchy(
  request: APIRequestContext,
): Promise<PlanFixture[]> {
  const rootResponse = await request.post("/api/repos/atlas/plans", {
    data: {
      title: "Atlas nested delivery",
      phases: [{ title: "Build atlas", description: "" }],
    },
  });
  expect(rootResponse.ok()).toBeTruthy();
  const plans: PlanFixture[] = [await rootResponse.json()];
  for (let depth = 1; depth <= 8; depth += 1) {
    const response = await request.post(
      `/api/plans/${plans.at(-1)!.id}/branches`,
      {
        data: {
          title: `Depth ${depth}: investigate regional map generation`,
          parent_phase_refs: ["1"],
          phases: [{ title: `Work at depth ${depth}`, description: "" }],
        },
      },
    );
    expect(response.ok()).toBeTruthy();
    plans.push(await response.json());
  }
  return plans;
}

async function openMainPlans(page: Page) {
  await page.addInitScript(() => {
    localStorage.setItem("sulion.sidebar.view.desktop.v1", "full");
    localStorage.setItem("sulion.sidebar.view.mobile.v1", "full");
  });
  await gotoApp(page);
  const drawerButton = page.getByRole("button", {
    name: "Open sessions drawer",
  });
  if (await drawerButton.isVisible()) await drawerButton.click();
  const group = page.locator('[data-repo-name="atlas"]');
  const toggle = group.locator(".sidebar__group-toggle");
  if ((await toggle.getAttribute("aria-expanded")) === "false")
    await toggle.click();
  return group.getByRole("tree", { name: "Plans for atlas", exact: true });
}

async function openPlans(page: Page) {
  await openMainPlans(page);
  await page
    .getByRole("button", { name: "Open plans for atlas", exact: true })
    .click();
}

test("shows the main Plans sidebar as a nested tree on desktop and mobile", async ({
  page,
  request,
}, testInfo) => {
  const plans = await createHierarchy(request);
  try {
    const tree = await openMainPlans(page);
    await expect(tree.getByRole("treeitem")).toHaveCount(9);
    const leaf = tree.getByRole("treeitem", {
      name: plans[8]!.title,
      exact: true,
    });
    await expect(leaf).toHaveAttribute("aria-level", "9");
    expect(await leaf.locator("..").getAttribute("role")).toBe("group");
    await expect(
      leaf.getByText(LEAF_PHASE_TITLE, { exact: true }),
    ).toBeVisible();
    await expect(leaf.getByRole("progressbar")).toHaveAttribute("max", "1");

    await tree
      .getByRole("button", { name: `Collapse ${plans[2]!.title}`, exact: true })
      .click();
    await expect(tree.getByRole("treeitem")).toHaveCount(3);
    await page.screenshot({
      path: testInfo.outputPath("desktop-main-plans.png"),
    });
    await tree
      .getByRole("button", { name: `Collapse ${plans[0]!.title}`, exact: true })
      .click();
    await expect(tree.getByRole("treeitem")).toHaveCount(1);
    await tree
      .getByRole("button", { name: `Expand ${plans[0]!.title}`, exact: true })
      .click();
    await expect(tree.getByRole("treeitem")).toHaveCount(3);
    await tree
      .getByRole("button", { name: `Expand ${plans[2]!.title}`, exact: true })
      .click();
    await leaf.locator(".plan-tree__body > strong").click();
    await expect(
      page
        .getByLabel("Selected plan")
        .getByText(LEAF_PHASE_TITLE, { exact: true }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Close", exact: true }).click();

    await page.setViewportSize({ width: 390, height: 844 });
    await page.getByRole("button", { name: "Open sessions drawer" }).click();
    const drawer = page.getByRole("complementary", { name: "Sessions" });
    await expect
      .poll(() =>
        drawer.evaluate((element) => element.getBoundingClientRect().x),
      )
      .toBe(0);
    await expect(tree.getByRole("treeitem")).toHaveCount(9);
    expect(
      await tree.evaluate(
        (element) => element.scrollWidth <= element.clientWidth,
      ),
    ).toBeTruthy();
    const disclosure = tree.getByRole("button", {
      name: `Collapse ${plans[0]!.title}`,
      exact: true,
    });
    const bounds = await disclosure.boundingBox();
    expect(bounds!.width).toBeGreaterThanOrEqual(44);
    expect(bounds!.height).toBeGreaterThanOrEqual(44);
    await tree
      .getByRole("button", { name: `Collapse ${plans[2]!.title}`, exact: true })
      .click();
    expect(
      await drawer.evaluate((element) => ({
        left: element.getBoundingClientRect().left,
        rightFits: element.getBoundingClientRect().right <= window.innerWidth,
        scrollLeft: element.scrollLeft,
        sidebarScrollLeft: element.querySelector(".sidebar")?.scrollLeft,
      })),
    ).toMatchObject({
      left: 0,
      rightFits: true,
      scrollLeft: 0,
      sidebarScrollLeft: 0,
    });
    expect(
      await tree.evaluate((element) => {
        const bounds = element.getBoundingClientRect();
        return Array.from(element.querySelectorAll(".plan-tree__body")).every(
          (row) => {
            const rect = row.getBoundingClientRect();
            return (
              rect.left >= bounds.left &&
              rect.right <= bounds.right &&
              row.scrollWidth <= row.clientWidth
            );
          },
        );
      }),
    ).toBeTruthy();
    await tree
      .getByText(plans[0]!.title, { exact: true })
      .scrollIntoViewIfNeeded();
    await page.screenshot({
      path: testInfo.outputPath("mobile-main-plans.png"),
    });
    for (const width of [320, 390]) {
      await page.setViewportSize({ width, height: 844 });
      expect(
        await tree.evaluate(
          (element) => element.scrollWidth <= element.clientWidth,
        ),
      ).toBeTruthy();
      expect(
        await tree
          .locator(".plan-tree__body > strong")
          .first()
          .evaluate((element) => getComputedStyle(element).whiteSpace),
      ).toBe("normal");
    }
    await tree
      .getByRole("button", { name: `Expand ${plans[2]!.title}`, exact: true })
      .click();
    await leaf.locator(".plan-tree__body > strong").click();
    await expect(tree).toBeHidden();
    await expect(
      page
        .getByLabel("Selected plan")
        .getByText(LEAF_PHASE_TITLE, { exact: true }),
    ).toBeVisible();
  } finally {
    for (const plan of [...plans].reverse()) {
      const response = await request.patch(`/api/plans/${plan.id}`, {
        data: { status: "canceled" },
      });
      expect(response.ok()).toBeTruthy();
    }
  }
});

test("navigates eight nested sub-plans on desktop and mobile without losing tree or draft state", async ({
  page,
  request,
}, testInfo) => {
  const plans = await createHierarchy(request);
  try {
    await openPlans(page);
    const tree = page.getByRole("tree", { name: "Plans", exact: true });
    const detail = page.getByLabel("Selected plan");
    const leaf = tree.getByRole("treeitem", {
      name: plans[8]!.title,
      exact: true,
    });
    await expect(leaf).toHaveAttribute("aria-level", "9");
    await leaf.locator(".plan-tree__row").first().click();
    await expect(
      detail.getByText(LEAF_PHASE_TITLE, { exact: true }),
    ).toBeVisible();
    await expect(tree).toBeVisible();
    await expect(leaf).toHaveAttribute("aria-selected", "true");
    await page.screenshot({
      path: testInfo.outputPath("desktop-plan-tree.png"),
    });

    await page
      .getByRole("button", { name: "Collapse all", exact: true })
      .click();
    await expect(tree.getByRole("treeitem")).toHaveCount(1);
    await page.getByRole("searchbox", { name: "Search plans" }).fill("Depth 8");
    await expect(tree.getByRole("treeitem")).toHaveCount(9);
    await expect(leaf).toBeVisible();
    await page.getByRole("searchbox", { name: "Search plans" }).clear();
    await expect(tree.getByRole("treeitem")).toHaveCount(1);
    await page.getByRole("button", { name: "Expand all", exact: true }).click();

    await page.setViewportSize({ width: 390, height: 844 });
    await expect(tree).toBeHidden();
    await expect(
      detail.getByText(LEAF_PHASE_TITLE, { exact: true }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Edit", exact: true }).click();
    await page
      .getByRole("textbox", { name: "Outcome", exact: true })
      .fill("Preserve my draft when browsing the tree");
    await page
      .getByRole("button", { name: "Back to plans", exact: true })
      .click();
    await expect(tree).toBeVisible();
    await expect(leaf).toHaveAttribute("aria-selected", "true");
    const disclosure = tree.getByRole("button", {
      name: "Collapse Atlas nested delivery",
      exact: true,
    });
    const bounds = await disclosure.boundingBox();
    expect(bounds!.width).toBeGreaterThanOrEqual(44);
    expect(bounds!.height).toBeGreaterThanOrEqual(44);
    await page.screenshot({
      path: testInfo.outputPath("mobile-plan-tree.png"),
    });
    await leaf.locator(".plan-tree__row").first().click();
    await expect(
      page.getByRole("textbox", { name: "Outcome", exact: true }),
    ).toHaveValue("Preserve my draft when browsing the tree");
    await page.screenshot({
      path: testInfo.outputPath("mobile-plan-detail.png"),
    });

    for (const width of [320, 390, 767, 768, 1024]) {
      await page.setViewportSize({ width, height: 844 });
      const surface = page.locator(".plan-modal .ui-overlay__surface");
      expect(
        await surface.evaluate(
          (element) => element.scrollWidth <= element.clientWidth,
        ),
      ).toBeTruthy();
      expect(
        await surface.evaluate(
          (element) =>
            element.getBoundingClientRect().bottom <= window.innerHeight,
        ),
      ).toBeTruthy();
      const reader = page.locator(".plan-modal__reader");
      expect(
        await reader.evaluate(
          (element) => element.scrollWidth <= element.clientWidth,
        ),
      ).toBeTruthy();
    }
    await page.setViewportSize({ width: 390, height: 844 });
    await page
      .getByRole("button", { name: "Back to plans", exact: true })
      .click();
    expect(
      await tree.evaluate(
        (element) => element.scrollWidth <= element.clientWidth,
      ),
    ).toBeTruthy();
  } finally {
    for (const plan of [...plans].reverse()) {
      const response = await request.patch(`/api/plans/${plan.id}`, {
        data: { status: "canceled" },
      });
      expect(response.ok()).toBeTruthy();
    }
  }
});
