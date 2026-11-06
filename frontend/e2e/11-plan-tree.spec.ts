import {
  expect,
  test,
  type APIRequestContext,
  type Page,
} from "@playwright/test";

import { gotoApp } from "./helpers";

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

async function openPlans(page: Page) {
  await page.addInitScript(() =>
    localStorage.setItem("sulion.sidebar.view.desktop.v1", "full"),
  );
  await gotoApp(page);
  const group = page.locator('[data-repo-name="atlas"]');
  const toggle = group.locator(".sidebar__group-toggle");
  if ((await toggle.getAttribute("aria-expanded")) === "false")
    await toggle.click();
  await group.getByRole("button", { name: "Open plans for atlas" }).click();
}

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
      detail.getByText("Work at depth 8", { exact: true }),
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
      detail.getByText("Work at depth 8", { exact: true }),
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
    await expect(page.getByRole("textbox", { name: "Outcome", exact: true })).toHaveValue(
      "Preserve my draft when browsing the tree",
    );
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
