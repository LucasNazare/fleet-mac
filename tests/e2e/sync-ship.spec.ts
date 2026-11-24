import { test, expect, type Page } from "@playwright/test";

const mock = (page: Page, fn: string, ...args: unknown[]) =>
  page.evaluate(([f, a]) => (window as any).__fleetMock[f as string](...(a as unknown[])), [fn, args] as const);

async function enableSync(page: Page) {
  await page.locator(".group-header").first().click({ button: "right" });
  await page.getByText("GitHub sync…").click();
  await page.getByTestId("sync-create-config").click();
  await page.getByTestId("yml-save").click();
  await expect(page.getByTestId("project-sync-pill")).toBeVisible();
}

test("local work and pre-sync sessions are kept as synced sessions", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("add-project").click();
  await page.keyboard.press("Meta+n"); // a session from before sync, in the main folder
  await enableSync(page);
  await mock(page, "localWork", 0, 26);
  const banner = page.getByTestId("local-work");
  await expect(banner).toContainText("26 unsaved changes");
  await expect(banner).toContainText("1 session from before sync");
  await page.getByTestId("keep-as-sessions").click();
  await page.getByTestId("dialog-ok").click();
  await expect(banner).toHaveCount(0);
  await expect(page.getByTestId("session-row")).toHaveCount(2);
  await expect(page.getByTestId("session-row").filter({ hasText: "Local changes" })).toHaveCount(1);
  await expect(page.getByTestId("sync-badge").first()).toBeVisible();
});

test("sessions from another Mac: Continue here, and Take over when it's active there", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("add-project").click();
  await enableSync(page);
  await mock(page, "otherSession", 0, {});
  await mock(page, "otherSession", 0, { branch: "fleet/px-pix", glyph: "PX", title: "Pix fee labels", activeOn: "iMac", activeAgo: "3m ago" });
  const rows = page.getByTestId("other-session");
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(1)).toContainText("active on iMac · 3m ago");
  await expect(rows.nth(0)).toContainText("iMac");
  await rows.nth(0).getByTestId("continue-here").click();
  await expect(page.getByTestId("session-row")).toHaveCount(1);
  await expect(page.getByTestId("session-row")).toContainText("Watermark missing on PDFs");
  await expect(rows).toHaveCount(1);
  // Active elsewhere → confirm Take over.
  await rows.nth(0).getByTestId("continue-here").click();
  await expect(page.getByRole("dialog")).toContainText("open on another Mac");
  await page.getByTestId("dialog-ok").click();
  await expect(page.getByTestId("session-row")).toHaveCount(2);
  await expect(rows).toHaveCount(0);
});

test("a session taken over elsewhere offers Take over", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("add-project").click();
  await enableSync(page);
  await page.keyboard.press("Meta+n");
  await mock(page, "syncStop", 0);
  await mock(page, "takenOver", 0, "iMac");
  const badge = page.getByTestId("sync-badge");
  await expect(badge).toContainText("active on iMac");
  await badge.click();
  await page.getByTestId("dialog-ok").click();
  await expect(badge).toHaveText("✓ pushed");
});

test("Ship: ordered branches, one conflicts, the rest ship and deploy; history + undo", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("add-project").click();
  await enableSync(page);
  for (let i = 0; i < 2; i++) {
    await page.keyboard.press("Meta+n");
    await mock(page, "syncStop", i);
  }
  await mock(page, "otherSession", 0, {});
  const conflicting = await page.evaluate(() => (window as any).__fleetMock.sessions[1].branch);
  await page.evaluate((b) => (window as any).__fleetMock.shipConflicts.add(b), conflicting);
  await page.keyboard.press("Meta+Shift+d");
  const sheet = page.getByTestId("ship-sheet");
  await expect(sheet.getByTestId("ship-row")).toHaveCount(3);
  await expect(sheet).toContainText("./deploy.sh all");
  await expect(page.getByTestId("ship-go")).toHaveText("Ship 3 & deploy");
  await page.getByTestId("ship-go").click();
  await expect(page.getByTestId("ship-summary")).toContainText("2 shipped");
  await expect(page.getByTestId("ship-summary")).toContainText("deploy-20261123-");
  await expect(page.getByTestId("ship-log")).toContainText("./deploy.sh all");
  // The conflicting session needs you; the shipped ones start fresh.
  await page.getByTestId("ship-sheet").getByRole("button", { name: "Close" }).click();
  await expect(page.locator('[data-testid="session-row"][data-state="needs_reply"]')).toHaveCount(1);
  await expect(page.locator('[data-badge="new"]')).toHaveCount(1);
  await expect(page.getByTestId("git-sync-header")).toContainText("last deploy");
  // History → undo creates a revert branch that shows up to ship.
  await page.getByTestId("ship-btn").click();
  await page.getByTestId("ship-history-tab").click();
  await expect(page.getByTestId("deploy-row").first()).toContainText("deploy-20261123-");
  await page.getByTestId("undo-feature").first().click();
  await page.getByTestId("dialog-ok").click();
  await expect(page.getByTestId("ship-row").filter({ hasText: "Undo:" })).toHaveCount(1);
});

test("a conflicting branch is fixed with Claude: from the Ship sheet, the badge, or a new session", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("add-project").click();
  await enableSync(page);
  await page.keyboard.press("Meta+n");
  await mock(page, "syncStop", 0);
  await mock(page, "otherSession", 0, {});
  const branch = await page.evaluate(() => (window as any).__fleetMock.sessions[0].branch);
  const other = "fleet/wm-watermark-missing";
  await page.evaluate((b) => b.forEach((x) => (window as any).__fleetMock.shipConflicts.add(x)), [branch, other]);
  await page.keyboard.press("Meta+Shift+d");
  await page.getByTestId("ship-go").click();
  await expect(page.getByTestId("ship-summary")).toContainText("0 shipped");
  // Its own session (idle, open) gets the fix prompt; the sheet closes onto it.
  await page.locator(`[data-testid="ship-row"][data-branch="${branch}"]`).getByTestId("fix-conflict").click();
  await expect(page.getByTestId("ship-sheet")).toHaveCount(0);
  const sent = () => page.evaluate(() => (window as any).__fleetMock.sent.map((x: any) => x.text) as string[]);
  await expect.poll(async () => (await sent()).join("\n")).toContain("conflicts with main in src/fee.ts");
  await expect(page.locator('[data-testid="session-row"][data-state="working"]')).toHaveCount(1);
  // The badge offers the same once it's idle again.
  await page.evaluate(() => { const m = (window as any).__fleetMock; m.sessions[0].state = "needs_reply"; });
  await mock(page, "retitle", 0, "Pix fee labels");
  const badge = page.locator('[data-badge="conflict"]');
  await expect(badge).toHaveClass(/clickable/);
  await badge.click();
  await expect.poll(async () => (await sent()).length).toBe(2);
  // A branch with no session on this Mac: a new session on it, with its identity.
  await page.getByTestId("ship-btn").click();
  await page.locator(`[data-testid="ship-row"][data-branch="${other}"]`).getByTestId("fix-conflict").click();
  await expect(page.getByTestId("session-row")).toHaveCount(2);
  await expect(page.getByTestId("other-session")).toHaveCount(0);
});

test("Ship never opens another project than the one you're in", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("add-project").click();
  await enableSync(page); // project-1 syncs
  await page.getByTestId("add-project").click(); // project-2 doesn't
  await page.keyboard.press("Meta+n"); // in the most recent project: project-2
  await expect(page.getByTestId("session-row")).toHaveCount(1);
  // project-2 has no GitHub repo: Ship opens its sync setup, which creates one.
  await mock(page, "noOriginAt", 1);
  await page.keyboard.press("Meta+Shift+d");
  await expect(page.getByTestId("ship-sheet")).toHaveCount(0);
  const setup = page.getByTestId("sync-setup");
  await expect(setup).toContainText("GitHub sync · project-2");
  await expect(page.getByTestId("sync-create-config")).toBeDisabled();
  await page.getByTestId("create-repo").click();
  await expect(page.getByTestId("repo-name")).toHaveValue("project-2");
  await page.getByTestId("create-repo-go").click();
  await expect(page.locator(".toast")).toContainText("Created https://github.com/dev/project-2 and pushed main (kept out: .env)");
  await expect(setup).toContainText("origin https://github.com/dev/demo.git");
  await expect(page.getByTestId("sync-create-config")).toBeEnabled();
});

test("cost guard shows the workflow change before applying it; add from GitHub", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("add-project").click();
  await page.evaluate(() => ((window as any).__fleetMock.workflows = [{ path: ".github/workflows/ci.yml", before: "on: push\n", after: "on:\n  push:\n    branches: [main]\n" }]));
  await enableSync(page);
  await page.locator(".group-header").first().click({ button: "right" });
  await page.getByText("GitHub sync status…").click();
  const guard = page.getByTestId("workflow-guard");
  await expect(guard).toContainText("ci.yml");
  await expect(page.getByTestId("workflow-apply")).toBeDisabled();
  await page.getByTestId("workflow-review").click();
  await expect(guard).toContainText("branches: [main]");
  await page.getByTestId("workflow-apply").click();
  await expect(page.getByText("Limited on fleet/config-20261123")).toBeVisible();
  await page.getByTestId("sync-setup").getByRole("button", { name: "Close" }).click();
  await page.getByTestId("add-from-github").click();
  await expect(page.getByTestId("repo-row")).toHaveCount(2);
  await page.getByTestId("repo-row").first().click();
  await expect(page.getByTestId("project-group")).toHaveCount(2);
});
