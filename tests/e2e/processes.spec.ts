import { test, expect } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("add-project").click();
});

test("detected scripts are offered, never started, and can be added", async ({ page }) => {
  const sugg = page.getByTestId("suggestions");
  await expect(sugg).toContainText("npm run dev");
  await sugg.getByRole("button", { name: "+ npm run dev" }).click();
  const row = page.getByTestId("process-row");
  await expect(row).toHaveCount(1);
  await expect(row).toHaveAttribute("data-status", "stopped");
  await expect(row).toContainText("■ stopped");
});

test("start shows the port as a link, stop frees it, crash turns red", async ({ page }) => {
  await page.getByRole("button", { name: "+ Add process" }).first().click();
  await page.getByTestId("proc-command").fill("npm run dev");
  await page.getByTestId("proc-save").click();

  const row = page.getByTestId("process-row");
  await row.click();
  await expect(page.getByTestId("process-view")).toBeVisible();
  await page.getByTestId("proc-start").click();
  await expect(row).toHaveAttribute("data-status", "running");
  await expect(row.locator(".port-link")).toHaveText(":5173");
  await expect(page.getByTestId("process-view")).toContainText("Open :5173");

  // ⌘. stops the selected process.
  await page.keyboard.press("Meta+Period");
  await expect(row).toHaveAttribute("data-status", "stopped");
  await expect(row.locator(".port-link")).toHaveCount(0);

  await page.getByTestId("proc-start").click();
  await page.evaluate(() => (window as any).__fleetMock.crash(0, 0, 1));
  await expect(row).toHaveAttribute("data-status", "crashed");
  await expect(row).toHaveClass(/crashed/);
  await expect(row).toContainText("✕ crashed (exit 1)");
});

test("send selected process output to a session", async ({ page }) => {
  await page.keyboard.press("Meta+Shift+n");
  await page.getByTestId("sheet-prompt").fill("fix the dev server");
  await page.getByTestId("sheet-start").click();

  await page.getByRole("button", { name: "+ Add process" }).first().click();
  await page.getByTestId("proc-command").fill("npm run dev");
  await page.getByTestId("proc-save").click();
  await page.getByTestId("process-row").click();
  await page.getByTestId("proc-start").click();
  await page.evaluate(() => (window as any).__fleetMock.crash(0, 0, 1));

  // xterm's selection isn't scriptable from the page, so use a DOM selection (the picker's fallback).
  await page.evaluate(() => {
    const pre = document.createElement("pre");
    pre.id = "sel-src";
    pre.textContent = "Error: Cannot find module 'left-pad'";
    document.body.appendChild(pre);
    const r = document.createRange();
    r.selectNodeContents(pre);
    const s = window.getSelection()!;
    s.removeAllRanges();
    s.addRange(r);
  });
  await page.keyboard.press("Meta+Shift+s");
  await expect(page.getByRole("dialog", { name: "Send to session" })).toBeVisible();
  await expect(page.locator(".send-preview")).toContainText("From npm run dev in project-");
  await page.getByTestId("send-target").first().click();
  const sent = await page.evaluate(() => (window as any).__fleetMock.sent);
  expect(sent).toHaveLength(1);
  expect(sent[0].text).toContain("Cannot find module 'left-pad'");
  await expect(page.locator(".hdr-title")).toHaveText("fix the dev server");
});
