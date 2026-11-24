import { test, expect, type Page } from "@playwright/test";

async function addSsh(page: Page, groupIndex: number, host: string, path: string) {
  await page.getByTestId("project-group").nth(groupIndex).getByTestId("add-ssh").click();
  await page.getByTestId("ssh-host").fill(host);
  await page.getByTestId("ssh-path").fill(path);
  await page.getByTestId("ssh-save").click();
}

test("⌃N opens a console under the session, + adds a shell, ⌃N hides it", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("add-project").click();
  await page.keyboard.press("Meta+n");
  await expect(page.getByTestId("session-view")).toBeVisible();
  await page.keyboard.press("Control+n");
  await expect(page.getByTestId("console-pane")).toBeVisible();
  await expect(page.getByTestId("console-tab")).toHaveCount(1);
  await page.getByTestId("console-add").click();
  await expect(page.getByTestId("console-tab")).toHaveCount(2);
  // Focus is in the console → ⌃N hides it.
  await page.locator(".console-host").click();
  await page.keyboard.press("Control+n");
  await expect(page.getByTestId("console-pane")).toHaveCount(0);
  // Consoles are per session: a new session starts without one.
  await page.keyboard.press("Meta+n");
  await expect(page.getByTestId("console-pane")).toHaveCount(0);
});

test("SSH: projects on the same server share one connection; clicking a remote row opens its shell", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("add-project").click();
  await page.getByTestId("add-project").click();
  await addSsh(page, 0, "deploy@prod", "~/api");
  await addSsh(page, 1, "deploy@prod", "~/web");
  const rows = page.getByTestId("remote-row");
  await expect(rows).toHaveCount(2);

  await rows.nth(0).getByTestId("remote-connect").click();
  await expect(rows.nth(0)).toHaveAttribute("data-state", "connected");
  await expect(rows.nth(1)).toHaveAttribute("data-state", "disconnected");
  await rows.nth(1).getByTestId("remote-connect").click();
  await expect(rows.nth(1)).toHaveAttribute("data-state", "connected");
  await expect(rows.nth(1)).toContainText("shared with 1 other project");
  const connects = await page.evaluate(() => (window as any).__fleetMock.sshConnects.length);
  expect(connects).toBe(1);

  await rows.nth(0).click();
  await expect(page.getByTestId("terminal-view")).toBeVisible();
  await expect(page.getByTestId("session-row")).toHaveCount(0);
  await expect(rows.nth(0)).toHaveClass(/active/);
});

test("SSH: the New session sheet can start claude on a server, connecting first", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("add-project").click();
  await addSsh(page, 0, "deploy@prod", "~/api");
  const row = page.getByTestId("remote-row");
  await expect(row).toHaveAttribute("data-state", "disconnected");

  await page.keyboard.press("Meta+Shift+n");
  await page.getByTestId("sheet-remote").selectOption({ index: 1 });
  await expect(page.getByTestId("sheet-worktree")).toHaveCount(0); // worktrees are local only
  await page.getByTestId("sheet-prompt").fill("check the logs");
  await page.getByTestId("sheet-start").click();

  await expect(row).toHaveAttribute("data-state", "connected");
  const session = page.getByTestId("session-row");
  await expect(session).toHaveCount(1);
  await expect(session.locator(".remote-badge")).toBeVisible();
});

test("SSH: right-click a remote row → New claude session", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("add-project").click();
  await addSsh(page, 0, "deploy@prod", "~/api");
  const row = page.getByTestId("remote-row");
  await row.click({ button: "right" });
  await page.getByRole("menuitem", { name: "New claude session" }).click();
  await expect(row).toHaveAttribute("data-state", "connected");
  await expect(page.getByTestId("session-row").locator(".remote-badge")).toBeVisible();
  await expect(page.getByTestId("session-view")).toBeVisible();
});
