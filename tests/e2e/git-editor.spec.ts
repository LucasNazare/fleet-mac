import { test, expect, type Page } from "@playwright/test";

async function setup(page: Page) {
  await page.goto("/");
  await page.getByTestId("add-project").click();
  await page.keyboard.press("Meta+Shift+n");
  await page.getByTestId("sheet-prompt").fill("fix fees");
  await page.getByTestId("sheet-start").click();
  await expect(page.getByTestId("git-panel")).toBeVisible();
}

test("stage → commit with Claude's message → push, all visible in Output", async ({ page }) => {
  await setup(page);
  const panel = page.getByTestId("git-panel");
  await expect(panel.getByTestId("change-row")).toHaveCount(2);
  await panel.getByTestId("change-row").filter({ hasText: "fee.ts" }).hover();
  await panel.getByTestId("change-row").filter({ hasText: "fee.ts" }).getByTestId("stage-btn").click();
  await expect(panel.getByTestId("staged-row")).toHaveCount(1);
  await panel.getByTestId("write-with-claude").click();
  await expect(panel.getByTestId("commit-msg")).toHaveValue("Use the Pix fee label in checkout");
  await panel.getByTestId("commit-btn").click();
  await expect(panel.getByTestId("staged-row")).toHaveCount(0);
  await expect(panel).toContainText("↑1");
  await panel.getByTestId("push-btn").click();
  await expect(panel).toContainText("↑0");
  await panel.getByTestId("output-toggle").click();
  const out = panel.getByTestId("output-list");
  await expect(out).toContainText("$ git add -A -- src/fee.ts");
  await expect(out).toContainText("$ git commit -m 'Use the Pix fee label in checkout'");
  await expect(out).toContainText("$ git push");
});

test("a rejected push shows a readable error with a one-click fix", async ({ page }) => {
  await setup(page);
  await page.evaluate(() => ((window as any).__fleetMock.rejectPush = true));
  await page.getByTestId("push-btn").click();
  const err = page.getByTestId("git-error");
  await expect(err).toContainText("Push was rejected");
  await expect(err.getByRole("button", { name: "Pull" })).toBeVisible();
});

test("overlap warning names both sessions", async ({ page }) => {
  await setup(page);
  await page.keyboard.press("Meta+Shift+n");
  await page.getByTestId("sheet-prompt").fill("labels");
  await page.getByTestId("sheet-start").click();
  await page.evaluate(() => { (window as any).__fleetMock.touch(0, "src/fee.ts"); (window as any).__fleetMock.touch(1, "src/fee.ts"); });
  await expect(page.getByTestId("overlap")).toContainText("fee.ts is being changed by");
  await expect(page.getByTestId("overlap")).toContainText("fix fees");
  await expect(page.getByTestId("overlap")).toContainText("labels");
});

test("clicking a change opens the diff with hunk staging", async ({ page }) => {
  await setup(page);
  await page.getByTestId("change-row").filter({ hasText: "fee.ts" }).click();
  await expect(page.getByTestId("diff-view")).toBeVisible();
  await page.getByTestId("stage-hunk").click();
  await expect(page.getByTestId("staged-row")).toHaveCount(1);
});

test("⌘P opens a file, edit, ⌘S saves", async ({ page }) => {
  await setup(page);
  const t0 = Date.now();
  await page.keyboard.press("Meta+p");
  await page.getByTestId("quick-open-input").fill("fee");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("editor-pane")).toBeVisible();
  await expect(page.locator(".monaco-editor .view-lines")).toContainText("export const fee = 2;");
  await page.locator(".monaco-editor .view-lines").click();
  await page.keyboard.press("Meta+ArrowDown");
  await page.keyboard.type("// edited");
  await expect(page.locator(".etab.on .dot")).toBeVisible();
  await page.keyboard.press("Meta+s");
  await expect(page.locator(".etab.on .dot")).toHaveCount(0);
  const saved = await page.evaluate(() => (window as any).__fleetMock.repo.files["src/fee.ts"].work);
  expect(saved).toContain("// edited");
  expect(Date.now() - t0).toBeLessThan(10_000);
  // The sidebar switched to the file tree.
  await expect(page.getByTestId("file-tree")).toBeVisible();
});

test("⌘I quick edit shows an inline diff to accept", async ({ page }) => {
  await setup(page);
  await page.keyboard.press("Meta+p");
  await page.getByTestId("quick-open-input").fill("fee");
  await page.keyboard.press("Enter");
  await page.locator(".monaco-editor .view-line", { hasText: "export const fee = 2;" }).click();
  await page.keyboard.press("Meta+i");
  await page.getByTestId("ie-input").fill("uppercase it");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("ie-accept")).toBeVisible();
  await expect(page.locator(".inline-removed")).toContainText("export const fee = 2;");
  await page.keyboard.press("Meta+Enter");
  await expect(page.getByTestId("inline-edit")).toHaveCount(0);
  await expect(page.locator(".monaco-editor .view-lines")).toContainText("EXPORT CONST FEE = 2;");
});

test("⌘K palette jumps to sessions and runs commands", async ({ page }) => {
  await setup(page);
  await page.keyboard.press("Meta+k");
  await page.getByTestId("palette-input").fill("toggle editor");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("editor-pane")).toBeVisible();
  await page.keyboard.press("Meta+k");
  await page.getByTestId("palette-input").fill("fix fees");
  await page.keyboard.press("Enter");
  await expect(page.locator(".hdr-title")).toHaveText("fix fees");
});

test("settings: recording a shortcut that's taken warns about the clash", async ({ page }) => {
  await setup(page);
  await page.keyboard.press("Meta+Comma");
  await page.getByRole("button", { name: "Shortcuts" }).click();
  const key = page.getByTestId("shortcut-quickOpen");
  await expect(key).toHaveText("⌘P");
  await key.click();
  await page.keyboard.press("Meta+j");
  await expect(key).toHaveText("⌘J");
  await expect(page.getByRole("alert").filter({ hasText: "Also used by: Jump to next session that needs you" })).toBeVisible();
});

test("find in files lists matches and opens them", async ({ page }) => {
  await setup(page);
  await page.keyboard.press("Meta+Shift+f");
  await page.getByTestId("ff-query").fill("label");
  await expect(page.getByTestId("find-in-files")).toContainText("1 result in 1 file");
  await page.locator(".ff-match").first().click();
  await expect(page.getByTestId("editor-pane")).toBeVisible();
});

test("Files shows the selected session's project", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("add-project").click();
  await page.getByTestId("add-project").click();
  const groups = page.getByTestId("project-group");
  await groups.nth(0).locator(".group-header .icon-btn").click();
  await groups.nth(1).locator(".group-header .icon-btn").click();
  const rows = page.getByTestId("session-row");
  await rows.nth(0).click();
  const first = await rows.nth(0).locator("xpath=ancestor::section").locator(".group-name").innerText();
  await page.getByTestId("mode-files").click();
  await expect(page.getByTestId("tree-project").locator("option:checked")).toHaveText(first);
  await page.getByTestId("mode-sessions").click();
  await rows.nth(1).click();
  const second = await rows.nth(1).locator("xpath=ancestor::section").locator(".group-name").innerText();
  expect(second).not.toBe(first);
  await page.getByTestId("mode-files").click();
  await expect(page.getByTestId("tree-project").locator("option:checked")).toHaveText(second);
});
