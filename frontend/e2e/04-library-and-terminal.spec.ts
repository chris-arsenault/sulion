import { expect, test } from "@playwright/test";

import {
  createMockTerminalSession,
  deleteSessionById,
  deleteLibraryEntries,
  expectTerminalToContain,
  gotoApp,
  openContextMenu,
  openSession,
  pressTerminalEnter,
  selectMenuItem,
} from "./helpers";

let mockSessionId: string | null = null;

test.afterEach(async ({ request }) => {
  if (mockSessionId) {
    await deleteSessionById(request, mockSessionId);
    mockSessionId = null;
  }
  await deleteLibraryEntries(request, "prompts");
  await deleteLibraryEntries(request, "references");
});

test("saves prompts and references from the timeline, opens refs, and injects prompts", async ({
  page,
  request,
}) => {
  mockSessionId = await createMockTerminalSession(request, "Atlas Mock Prompt");

  await gotoApp(page);
  await openSession(page, "Atlas Claude");

  await page.getByTestId("turn-row").first().click();

  await openContextMenu(page.getByLabel("Prompt actions"));
  await selectMenuItem(page, "Save as prompt");
  const promptRow = page.locator(".lib-sec__entry").filter({
    hasText: "PROMPT_TIMELINE_SENTINEL",
  });
  await expect(promptRow).toBeVisible();

  const assistantBlock = page.getByLabel("Assistant block actions").first();
  await openContextMenu(assistantBlock);
  await selectMenuItem(page, "Save as reference");
  const referenceRow = page.locator(".lib-sec__entry").filter({
    hasText: "Inspecting src/lib.rs before editing.",
  });
  await expect(referenceRow).toBeVisible();

  await openSession(page, "Atlas Mock Prompt");
  await promptRow.first().click();
  await pressTerminalEnter(page);
  await expectTerminalToContain(page, "MOCK_ECHO printf 'PROMPT_TIMELINE_SENTINEL\\n'");

  await referenceRow.first().click();
  await expect(page.getByTestId("ref-tab")).toContainText(
    "Inspecting src/lib.rs before editing.",
  );
});
