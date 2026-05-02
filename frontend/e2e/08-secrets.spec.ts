import { expect, test } from "@playwright/test";

import {
  createLabeledSession,
  deleteSessionById,
  expectTerminalToContain,
  gotoApp,
  openContextMenu,
  openSession,
  runTerminalCommand,
  tab,
} from "./helpers";

let sessionId: string | null = null;

test.afterEach(async ({ request }) => {
  if (!sessionId) return;
  await deleteSessionById(request, sessionId);
  sessionId = null;
});

test("creates a key/value secret, grants it from a terminal tab, redeems it, and revokes it", async ({
  page,
  request,
}) => {
  const suffix = String(Date.now());
  const label = `Secrets Shell ${suffix.slice(-5)}`;
  const secretId = `e2e-secret-${suffix}`;
  const secretValue = `sulion-secret-value-${suffix}`;
  sessionId = await createLabeledSession(request, "atlas", label);

  await gotoApp(page);
  await openSession(page, label);

  await page.getByRole("button", { name: "Open secrets manager" }).click();
  await expect(page.getByRole("tab", { name: "secrets" })).toBeVisible();
  await expect(page.getByText("Secret editor")).toBeVisible();

  await page.getByLabel("ID").fill(secretId);
  await page.getByLabel("Description").fill("E2E env bundle");
  await page.getByPlaceholder("ANTHROPIC_API_KEY").fill("E2E_SECRET_VALUE");
  await page.getByPlaceholder("value").fill(secretValue);
  await page.getByRole("button", { name: "Save" }).click();

  await expect(page.getByText(`Saved ${secretId}`)).toBeVisible();
  await expect(page.getByText("Blank existing values are kept; enter a value to overwrite.")).toBeVisible();
  await expect(page.getByDisplayValue(secretValue)).toHaveCount(0);

  await openContextMenu(tab(page, "terminal", label));
  await page.getByRole("menuitem", { name: "Secrets" }).hover();
  await page.getByRole("menuitem", { name: "Enable secret" }).hover();
  await page.getByRole("menuitem", { name: secretId }).hover();
  await page.getByRole("menuitem", { name: "with-cred" }).hover();
  await page.getByRole("menuitem", { name: "10m" }).click();

  await openContextMenu(page.locator(`[data-session-name="${label}"]`));
  await page.getByRole("menuitem", { name: "Secrets" }).hover();
  await page.getByRole("menuitem", { name: "Active secrets" }).hover();
  await expect(page.getByRole("menuitem", { name: new RegExp(`${secretId} · with-cred`) })).toBeVisible();
  await page.keyboard.press("Escape");

  await tab(page, "terminal", label).click();
  await runTerminalCommand(
    page,
    `with-cred ${secretId} -- sh -lc 'printf "E2E_SECRET_VALUE=$E2E_SECRET_VALUE\\n"'`,
  );
  await expectTerminalToContain(page, `E2E_SECRET_VALUE=${secretValue}`);

  await openContextMenu(page.locator(`[data-session-name="${label}"]`));
  await page.getByRole("menuitem", { name: "Secrets" }).hover();
  await page.getByRole("menuitem", { name: "Active secrets" }).hover();
  await page.getByRole("menuitem", { name: new RegExp(`${secretId} · with-cred`) }).click();

  await tab(page, "terminal", label).click();
  await runTerminalCommand(
    page,
    `with-cred ${secretId} -- sh -lc 'printf "SHOULD_NOT_PRINT=$E2E_SECRET_VALUE\\n"'`,
  );
  await expectTerminalToContain(page, "credential-helper: broker denied access");
});
