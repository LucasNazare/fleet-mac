import { test, expect } from "@playwright/test";

const THEMES = ["dark", "light", "hacker", "cyberpunk", "glass", "pear", "retro", "soft"];

/** Contrast of CSS tokens, computed in the page (translucent colors composited over a dark backdrop for Glass). */
async function contrasts(page: import("@playwright/test").Page) {
  return page.evaluate(() => {
    const css = getComputedStyle(document.documentElement);
    const v = (n: string) => css.getPropertyValue(`--${n}`).trim();
    const parse = (c: string): [number, number, number, number] => {
      const el = document.createElement("div");
      el.style.color = c;
      document.body.appendChild(el);
      const m = getComputedStyle(el).color.match(/[\d.]+/g)!.map(Number);
      el.remove();
      return [m[0], m[1], m[2], m[3] ?? 1];
    };
    const over = (top: [number, number, number, number], under: [number, number, number]) =>
      [0, 1, 2].map((i) => top[i] * top[3] + under[i] * (1 - top[3])) as [number, number, number];
    const lum = ([r, g, b]: number[]) => {
      const f = (x: number) => { const c = x / 255; return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; };
      return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
    };
    const ratio = (a: number[], b: number[]) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };
    const backdrop: [number, number, number] = [28, 30, 40];
    const base = over(parse(v("app-bg") === "transparent" ? "rgba(0,0,0,0)" : v("app-bg")), backdrop);
    const panel = over(parse(v("panel")), base);
    const bg = over(parse(v("bg")), base);
    const text = over(parse(v("text")), panel);
    const out: Record<string, number> = {
      "text/panel": ratio(text, panel),
      "text/bg": ratio(over(parse(v("text")), bg), bg),
      "muted/panel": ratio(over(parse(v("muted")), panel), panel),
    };
    for (const s of ["needs", "working", "done", "failed", "idle"]) out[`state-${s}/panel`] = ratio(over(parse(v(`state-${s}`)), panel), panel);
    // Chip text on every session color.
    const chips = [...document.querySelectorAll<HTMLElement>(".chip")];
    out["chips"] = chips.length ? Math.min(...chips.map((c) => ratio(parse(getComputedStyle(c).color).slice(0, 3), parse(getComputedStyle(c).backgroundColor).slice(0, 3)))) : 99;
    return out;
  });
}

test("every theme applies everywhere and stays readable", async ({ page }) => {
  test.setTimeout(90_000);
  await page.setViewportSize({ width: 1400, height: 860 });
  await page.goto("/");
  await page.getByTestId("add-project").click();
  for (let i = 0; i < 4; i++) await page.keyboard.press("Meta+n");
  await page.evaluate(() => {
    const m = (window as any).__fleetMock;
    m.setState(1, "needs_reply", "deploy now?");
    m.setState(2, "working");
    m.setState(3, "done");
  });
  const termBgs = new Set<string>();
  for (const id of THEMES) {
    await page.keyboard.press("Meta+Comma");
    await page.locator(`[data-theme-id="${id}"]`).click();
    await expect(page.locator("html")).toHaveAttribute("data-theme", id);
    await page.mouse.click(5, 300);
    await expect(page.locator(".settings")).toHaveCount(0);
    await page.waitForTimeout(250);
    const c = await contrasts(page);
    expect(c["text/panel"], `${id} text/panel`).toBeGreaterThanOrEqual(4.5);
    expect(c["text/bg"], `${id} text/bg`).toBeGreaterThanOrEqual(4.5);
    expect(c["muted/panel"], `${id} muted/panel`).toBeGreaterThanOrEqual(3);
    for (const s of ["needs", "working", "done", "failed", "idle"]) expect(c[`state-${s}/panel`], `${id} state-${s}`).toBeGreaterThanOrEqual(3);
    expect(c["chips"], `${id} chip text`).toBeGreaterThanOrEqual(4.5);
    termBgs.add(await page.evaluate(() => getComputedStyle(document.querySelector(".term-host .xterm-viewport") ?? document.body).backgroundColor));
    await page.screenshot({ path: `test-results/themes/${id}.png` });
  }
  expect(termBgs.size, "terminal background follows the theme").toBeGreaterThanOrEqual(6);
});

test("effects can be turned off and the palette switches themes", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("add-project").waitFor();
  await page.keyboard.press("Meta+Comma");
  await expect(page.getByTestId("theme-card")).toHaveCount(8);
  await page.locator(".settings").screenshot({ path: "test-results/themes/gallery.png" });
  await page.getByTestId("effects-select").selectOption("off");
  await expect(page.locator("html")).toHaveAttribute("data-effects", "off");
  await page.mouse.click(5, 300);
  await page.keyboard.press("Meta+k");
  await page.keyboard.type("Theme: Cyberpunk");
  await page.keyboard.press("Enter");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "cyberpunk");
});
