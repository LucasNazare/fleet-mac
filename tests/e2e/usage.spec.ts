import { test, expect } from "@playwright/test";

test("session header shows cost, sub-agent share and turn time; usage sheet groups by project", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("add-project").click();
  await page.keyboard.press("Meta+Shift+n");
  await page.getByTestId("sheet-prompt").fill("price the checkout");
  await page.getByTestId("sheet-start").click();

  const t = (cost: number) => ({ input: 1000, output: 500, cacheRead: 90_000, cacheWrite: 8_000, cost, unpriced: 0 });
  await page.evaluate((m) => (window as any).__fleetMock.setMetrics(0, m), {
    main: t(0.75), sub: t(0.25), turns: 2, turnMs: 200_000, waitMs: 20_000, lastTurnMs: 125_000, lastWaitMs: 5_000, turnStarted: null,
  });
  await expect(page.getByTestId("session-cost")).toHaveText("$1.00 · 25% agents");
  await expect(page.getByTestId("turn-time")).toHaveText("◷ 2m 05s");

  // A turn in progress ticks live.
  await page.evaluate(() => (window as any).__fleetMock.setMetrics(0, { turnStarted: Date.now() - 42_000 }));
  await expect(page.getByTestId("turn-time")).toHaveText(/◷ 4[2-5]s/);

  // Status bar total → usage sheet.
  await page.getByTestId("usage-today").click();
  await expect(page.getByTestId("usage-sheet")).toBeVisible();
  await expect(page.getByTestId("usage-projects")).toContainText("25%");
  await expect(page.getByTestId("usage-projects")).toContainText("2m 00s"); // avg turn 480s / 4
  await expect(page.getByTestId("usage-days").locator("tbody tr")).toHaveCount(3);
  await page.getByTestId("usage-sheet").getByRole("button", { name: "Today", exact: true }).click();
  await expect(page.getByTestId("usage-total")).toHaveText("$2.00");
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("usage-sheet")).toHaveCount(0);
});
