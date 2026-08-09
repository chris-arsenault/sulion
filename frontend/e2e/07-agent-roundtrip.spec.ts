import { expect, test } from "@playwright/test";

import {
  createLabeledSession,
  deleteSessionById,
  expectTerminalToContain,
  gotoApp,
  openSession,
  runTerminalCommand,
} from "./helpers";

const roundtripCases = [
  {
    label: "Atlas Claude Mock Agent",
    command: "sulion-agent --type claude --mode mock --",
    prompt: "validate claude ingest parity",
    totalEvents: 7,
    assistantText: "Claude mock assistant processed: validate claude ingest parity",
    subagentText: "Subagent report: Claude mock parity validated.",
  },
  {
    label: "Atlas Codex Mock Agent",
    command: "sulion-agent --type codex --mode mock --",
    prompt: "validate codex ingest parity",
    totalEvents: 14,
    assistantText: "Codex mock assistant processed: validate codex ingest parity",
    subagentText: "Subagent report: Codex mock parity validated.",
    sidechainPrompt: "Inspect parity findings in a subagent",
  },
] as const;

const sessionIds = new Map<string, string>();

test.afterEach(async ({ request }) => {
  for (const sessionId of sessionIds.values()) {
    await deleteSessionById(request, sessionId);
  }
  sessionIds.clear();
});

for (const scenario of roundtripCases) {
  test(`streams a ${scenario.label} mock transcript through ingest into the timeline`, async ({
    page,
    request,
  }) => {
    const sessionId = await createLabeledSession(request, "atlas", scenario.label);
    sessionIds.set(scenario.label, sessionId);

    await gotoApp(page);
    await openSession(page, scenario.label);

    await runTerminalCommand(page, scenario.command);
    await expectTerminalToContain(page, "Type a prompt and press Enter.");

    await runTerminalCommand(page, scenario.prompt);
    await expectTerminalToContain(page, "wrote");

    await expect
      .poll(async () => page.getByTestId("turn-row").count(), {
        message: `expected timeline turns for ${scenario.label}`,
        timeout: 20_000,
      })
      .toBeGreaterThan(0);

    await expect(page.getByTestId("timeline-pane")).toContainText(`${scenario.totalEvents} events`);

    await page
      .getByTestId("turn-row")
      .filter({ hasText: scenario.prompt })
      .first()
      .click();
    await expect(page.getByTestId("turn-detail")).toContainText(scenario.assistantText);

    await expect(page.locator('[data-testid="tool-pair-row"][data-tool-type="edit"]')).toBeVisible();

    if (!("sidechainPrompt" in scenario)) {
      await expect(
        page.locator('[data-testid="tool-pair-row"][data-tool-type="web_search"]'),
      ).toBeVisible();
      await expect(
        page.locator('[data-testid="tool-pair-row"][data-tool-type="task"]'),
      ).toBeVisible();

      await page.getByRole("button", { name: /View agent log/i }).click();
      await expect(page.getByTestId("subagent-modal")).toContainText(scenario.subagentText);
      return;
    }

    await page.getByRole("button", { name: "sidechain" }).click();
    await page
      .getByTestId("turn-row")
      .filter({ hasText: scenario.sidechainPrompt })
      .first()
      .click();
    await expect(
      page.locator('[data-testid="tool-pair-row"][data-tool-type="web_search"]'),
    ).toBeVisible();
    await expect(page.locator('[data-testid="tool-pair-row"][data-tool-type="task"]')).toBeVisible();
    await page.getByRole("button", { name: /View agent log/i }).click();
    await expect(page.getByTestId("subagent-modal")).toContainText(scenario.subagentText);
  });
}
