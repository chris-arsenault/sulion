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

test("creates single-line and multiline secret values, redeems them, and revokes the grant", async ({
  page,
  request,
}) => {
  const suffix = String(Date.now());
  const label = `Secrets Shell ${suffix.slice(-5)}`;
  const secretId = `e2e-secret-${suffix}`;
  const secretValue = `sulion-secret-value-${suffix}`;
  const sshPrivateKey = [
    "-----BEGIN OPENSSH PRIVATE KEY-----",
    "ZmFrZS1vcGVuc3NoLWtleQ==",
    "-----END OPENSSH PRIVATE KEY-----",
    "",
  ].join("\n");
  sessionId = await createLabeledSession(request, "atlas", label);

  await gotoApp(page);
  await openSession(page, label);

  await page.getByRole("button", { name: "Open secrets manager" }).click();
  await expect(page.getByRole("tab", { name: "secrets" }).first()).toBeVisible();
  await expect(page.getByRole("heading", { name: "New bundle" })).toBeVisible();

  // exact: the rail's "Unpin sidebar" / "Resize sidebar" labels substring-
  // match "ID" otherwise.
  await page.getByLabel("ID", { exact: true }).fill(secretId);
  await page.getByLabel("Description").fill("E2E env bundle");
  await page.getByPlaceholder("ANTHROPIC_API_KEY").fill("E2E_SECRET_VALUE");
  await page.getByPlaceholder("value").fill(secretValue);
  await page.getByRole("button", { name: "Add pair" }).click();
  await page.locator('input[value="NEW_KEY_1"]').fill("SSH_PRIVATE_KEY");
  await page.getByPlaceholder("value").last().fill(sshPrivateKey);
  await page.getByRole("button", { name: "Save" }).click();

  await expect(page.getByText(`Saved ${secretId}`)).toBeVisible();
  await expect(page.getByText("Blank existing values are kept; enter a value to overwrite.")).toBeVisible();
  // The saved value must not remain displayed in any input. (Live input
  // values are properties, not attributes, so check them via the DOM.)
  await expect
    .poll(async () =>
      page
        .locator("input, textarea")
        .evaluateAll(
          (els, value) =>
            els.filter((el) => (el as HTMLInputElement).value === value).length,
          secretValue,
        ),
    )
    .toBe(0);

  await openContextMenu(tab(page, "terminal", label));
  await page.getByRole("menuitem", { name: "Secrets" }).hover();
  await page.getByRole("menuitem", { name: "Enable secret" }).hover();
  await page.getByRole("menuitem", { name: secretId }).hover();
  await page.getByRole("menuitem", { name: "10m" }).click();

  await openContextMenu(page.locator(`[data-session-name="${label}"]`));
  await page.getByRole("menuitem", { name: "Secrets" }).hover();
  await page.getByRole("menuitem", { name: "Active secrets" }).hover();
  await expect(page.getByRole("menuitem", { name: new RegExp(`${secretId} ·`) })).toBeVisible();
  await page.keyboard.press("Escape");

  await tab(page, "terminal", label).click();
  await runTerminalCommand(
    page,
    `with-cred ${secretId} -- sh -lc 'printf "E2E_SECRET_VALUE=$E2E_SECRET_VALUE\\n"'`,
  );
  await expectTerminalToContain(page, `E2E_SECRET_VALUE=${secretValue}`);
  await runTerminalCommand(
    page,
    `with-cred ${secretId} -- sh -lc 'key_lines=$(printf "%s" "$SSH_PRIVATE_KEY" | wc -l); printf "SSH_KEY_LINES=%s\\n" "$key_lines"'`,
  );
  await expectTerminalToContain(page, "SSH_KEY_LINES=3");

  await openContextMenu(page.locator(`[data-session-name="${label}"]`));
  await page.getByRole("menuitem", { name: "Secrets" }).hover();
  await page.getByRole("menuitem", { name: "Active secrets" }).hover();
  await page.getByRole("menuitem", { name: new RegExp(`${secretId} ·`) }).click();

  await tab(page, "terminal", label).click();
  await runTerminalCommand(
    page,
    `with-cred ${secretId} -- sh -lc 'printf "SHOULD_NOT_PRINT=$E2E_SECRET_VALUE\\n"'`,
  );
  await expectTerminalToContain(page, "credential-helper: broker denied access");
});
