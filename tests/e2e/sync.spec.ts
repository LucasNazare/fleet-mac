import { test, expect, type Page } from "@playwright/test";

const mock = (page: Page, fn: string, ...args: unknown[]) =>
  page.evaluate(([f, a]) => (window as any).__fleetMock[f as string](...(a as unknown[])), [fn, args] as const);

async function enableSync(page: Page) {
  await page.goto("/");
  await page.getByTestId("add-project").click();
  await page.locator(".group-header").first().click({ button: "right" });
  await page.getByText("GitHub sync…").click();
  await expect(page.getByTestId("sync-setup")).toContainText("Signed in as dev");
  await page.getByTestId("sync-create-config").click();
  await expect(page.getByTestId("config-editor")).toBeVisible();
  await expect(page.getByTestId("yml-errors")).toContainText("valid");
  await page.getByTestId("yml-save").click();
  await expect(page.getByTestId("config-editor")).toHaveCount(0);
  await expect(page.getByTestId("project-sync-pill")).toBeVisible();
}

test("GitHub sync: setup → fleet.yml → a finished prompt is pushed to its own branch", async ({ page }) => {
  await enableSync(page);
  await expect(page.getByTestId("sync-status")).toContainText("synced");
  await page.keyboard.press("Meta+n");
  const badge = page.getByTestId("sync-badge");
  await expect(badge).toHaveAttribute("data-badge", "new");
  await mock(page, "syncStop", 0);
  await expect(badge).toHaveAttribute("data-badge", "pushed");
  await expect(badge).toHaveText("✓ pushed");
  // The automatic git commands are in the Output drawer's sync log.
  await page.getByTestId("sync-status").click();
  await page.getByText("Sync log").click();
  await expect(page.getByTestId("output-list")).toContainText("git push --porcelain --atomic origin refs/heads/fleet/");
  await expect(page.getByTestId("output-list")).toContainText("fleet-state");
  // main is production: the panel says so, and Push on main is disabled.
  await expect(page.getByTestId("git-sync-header")).toContainText("main = production");
  await expect(page.getByTestId("git-sync-header")).toContainText("settings go live with the next Ship");
});

test("GitHub sync: offline queues pushes and drains when back", async ({ page }) => {
  await enableSync(page);
  await page.keyboard.press("Meta+n");
  await mock(page, "setOffline", true);
  await mock(page, "syncStop", 0);
  const badge = page.getByTestId("sync-badge");
  await expect(badge).toHaveText("↑ 1 waiting");
  await expect(page.getByTestId("sync-status")).toContainText("offline · 1 waiting");
  await mock(page, "setOffline", false);
  await expect(badge).toHaveText("✓ pushed");
  await expect(page.getByTestId("sync-status")).toContainText("synced");
});

test("GitHub sync: pause stops it; excluded files are flagged", async ({ page }) => {
  await enableSync(page);
  await page.keyboard.press("Meta+n");
  await mock(page, "syncStop", 0);
  await mock(page, "excluded", 0, [".env.local"]);
  await expect(page.getByTestId("excluded-files")).toContainText(".env.local");
  await page.getByTestId("sync-status").click();
  await page.getByTestId("pause-all").click();
  await expect(page.getByTestId("sync-status")).toContainText("sync paused");
  await expect(page.getByTestId("sync-badge")).toHaveText("⏸ paused");
});

test("fleet.yml: errors show with line numbers, live and in the git panel", async ({ page }) => {
  await enableSync(page);
  await page.locator(".group-header").first().click({ button: "right" });
  await page.getByText("Edit fleet.yml…").click();
  const text = page.getByTestId("yml-text");
  await text.fill("project: demo\nsync:\n  enabeld: true\n  fetchIntervalSec: soon\n");
  await expect(page.getByTestId("yml-errors")).toContainText("line 3:");
  await expect(page.getByTestId("yml-errors")).toContainText("line 4: sync.fetchIntervalSec");
  await expect(page.getByTestId("yml-save")).toBeDisabled();
  await page.keyboard.press("Escape");
  // A broken fleet.yml on main: listed in the git panel, fleet keeps the last valid config.
  await page.keyboard.press("Meta+n");
  await mock(page, "brokenConfig", 0, 7, "processes[0]: \"command\" is a required property");
  await expect(page.getByTestId("fleet-yml-errors")).toContainText("line 7: processes[0]");
  await expect(page.getByTestId("fleet-yml-errors")).toContainText("last valid version");
});
