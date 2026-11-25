import { test, expect, type Page } from "@playwright/test";

async function addProject(page: Page) {
  await page.getByTestId("add-project").click();
}

async function newSession(page: Page, prompt: string) {
  await page.keyboard.press("Meta+Shift+n");
  await page.getByTestId("sheet-prompt").fill(prompt);
  await page.getByTestId("sheet-start").click();
}

test("add projects, start sessions, switch between them", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByText("Add a project to start")).toBeVisible();
  await addProject(page);
  await addProject(page);
  await expect(page.getByTestId("project-group")).toHaveCount(2);

  await newSession(page, "Pix fee labels on checkout");
  await newSession(page, "Refactor auth middleware");
  await expect(page.getByTestId("session-row")).toHaveCount(2);

  // Unique glyphs and colours.
  const glyphs = await page.locator(".session-row .chip").allInnerTexts();
  expect(new Set(glyphs).size).toBe(2);

  // Active session header + terminal output from the mock PTY.
  await expect(page.locator(".hdr-title")).toHaveText("Refactor auth middleware");
  await expect(page.locator(".term-host .xterm")).toBeVisible();

  // ⌘[ goes to the previous session.
  await page.keyboard.press("Meta+BracketLeft");
  await expect(page.locator(".hdr-title")).toHaveText("Pix fee labels on checkout");
  await expect(page).toHaveTitle(/· Pix fee labels on checkout — project-/);
});

test("state changes show in the row, status bar and ⌘J", async ({ page }) => {
  await page.goto("/");
  await addProject(page);
  await newSession(page, "first task");
  await newSession(page, "second task");

  await page.evaluate(() => (window as any).__fleetMock.setState(0, "needs_permission", "run ./deploy.sh all?"));
  const row = page.getByTestId("session-row").filter({ hasText: "first task" });
  await expect(row).toHaveAttribute("data-state", "needs_permission");
  await expect(row).toContainText("● needs you · run ./deploy.sh all?");
  await expect(page.getByTestId("count-needs")).toContainText("1 need you");

  // ⌘J jumps to the session that needs you.
  await page.keyboard.press("Meta+j");
  await expect(page.locator(".hdr-title")).toHaveText("first task");
  await expect(page.locator(".question-bar")).toContainText("run ./deploy.sh all?");

  // Clicking the count filters the sidebar.
  await page.getByTestId("count-needs").click();
  await expect(page.getByTestId("session-row")).toHaveCount(1);
  await page.getByTestId("count-needs").click();
  await expect(page.getByTestId("session-row")).toHaveCount(2);

  // Live retitle keeps history.
  await page.evaluate(() => (window as any).__fleetMock.retitle(0, "Deploy all environments"));
  await expect(page.locator(".hdr-title")).toHaveText("Deploy all environments");
  await expect(page.locator(".hdr-title")).toHaveAttribute("title", /was: first task/);
});

test("split view and panel toggles", async ({ page }) => {
  await page.goto("/");
  await addProject(page);
  await newSession(page, "alpha");
  await newSession(page, "beta");
  await page.keyboard.press("Meta+Backslash");
  await expect(page.getByTestId("session-view")).toHaveCount(2);
  await page.keyboard.press("Meta+b");
  await expect(page.locator(".sidebar")).toHaveCount(0);
  await page.keyboard.press("Meta+b");
  await expect(page.locator(".sidebar")).toHaveCount(1);
  await page.keyboard.press("Meta+Alt+b");
  await expect(page.locator(".right-panel")).toHaveCount(0);
});

test("rename by double-click locks the title", async ({ page }) => {
  await page.goto("/");
  await addProject(page);
  await newSession(page, "old name");
  await page.locator(".row-title").dblclick();
  await page.locator(".inline-input").fill("New name");
  await page.locator(".inline-input").press("Enter");
  await expect(page.locator(".row-title")).toContainText("New name");
  await expect(page.locator(".row-title")).toContainText("pinned");
});

test("switching between 10 live terminals stays under 100 ms", async ({ page }) => {
  await page.goto("/");
  for (let i = 0; i < 3; i++) await addProject(page);
  for (let i = 0; i < 10; i++) {
    // "+" on a project header starts a session there directly.
    await page.locator(".group-header .icon-btn").nth(i % 3).click();
    await expect(page.getByTestId("session-row")).toHaveCount(i + 1);
  }
  await expect(page.getByTestId("session-row")).toHaveCount(10);
  const glyphs = await page.locator(".session-row .chip").allInnerTexts();
  expect(new Set(glyphs).size).toBe(10);

  const times: number[] = await page.evaluate(async () => {
    const out: number[] = [];
    const rows = Array.from(document.querySelectorAll<HTMLElement>("[data-testid=session-row]"));
    for (let round = 0; round < 2; round++) {
      for (const r of rows) {
        const t0 = performance.now();
        r.click();
        await new Promise((res) => requestAnimationFrame(res));
        out.push(performance.now() - t0);
      }
    }
    return out;
  });
  times.sort((a, b) => a - b);
  const p95 = times[Math.floor(times.length * 0.95) - 1];
  console.log(`switch times ms: median ${times[times.length >> 1].toFixed(1)}, p95 ${p95.toFixed(1)}`);
  expect(p95).toBeLessThan(100);
});

test("⌘N starts a session immediately in the current project (no dialog)", async ({ page }) => {
  await page.goto("/");
  await addProject(page);
  await page.keyboard.press("Meta+n");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.getByTestId("session-row")).toHaveCount(1);
  await expect(page.getByTestId("session-view")).toBeVisible();
  await page.keyboard.press("Meta+n");
  await expect(page.getByTestId("session-row")).toHaveCount(2);
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("⌘G toggles the git panel", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator(".right-panel")).toHaveCount(1);
  await page.keyboard.press("Meta+g");
  await expect(page.locator(".right-panel")).toHaveCount(0);
  await page.keyboard.press("Meta+g");
  await expect(page.locator(".right-panel")).toHaveCount(1);
});

test("uppercase, space and punctuation are typed once", async ({ page }) => {
  await page.goto("/");
  await addProject(page);
  await page.keyboard.press("Meta+n");
  const ta = page.locator(".term-host .xterm-helper-textarea").first();
  await expect(ta).toBeAttached();
  await ta.focus();
  const typed = () => page.evaluate(() => (window as any).__fleetMock.ptyWrites.map((w: any) => w.data).join(""));
  const start = (await typed()).length;
  await page.keyboard.type("AA Bb, let's \"go\"!");
  await expect.poll(async () => (await typed()).slice(start)).toBe("AA Bb, let's \"go\"!");
});

test("dead keys: ' then s after a commit that repeats it types 's once", async ({ page }) => {
  await page.goto("/");
  await addProject(page);
  await page.keyboard.press("Meta+n");
  const ta = page.locator(".term-host .xterm-helper-textarea").first();
  await expect(ta).toBeAttached();
  await ta.focus();
  const typed = () => page.evaluate(() => (window as any).__fleetMock.ptyWrites.map((w: any) => w.data).join(""));
  const start = (await typed()).length;
  await page.evaluate(() => {
    const t = document.activeElement as HTMLTextAreaElement;
    const key = (k: string, composing: boolean) => {
      const e = new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, isComposing: composing });
      Object.defineProperty(e, "keyCode", { value: 229 });
      t.dispatchEvent(e);
    };
    const comp = (type: string, data: string) => t.dispatchEvent(new CompositionEvent(type, { data, bubbles: true }));
    const input = (inputType: string, data: string) => t.dispatchEvent(new InputEvent("input", { inputType, data, bubbles: true }));
    key("Dead", false); comp("compositionstart", ""); comp("compositionupdate", "'"); input("insertCompositionText", "'");
    key("s", true); comp("compositionend", "'"); input("insertText", "'s");
  });
  await expect.poll(async () => (await typed()).slice(start)).toBe("'s");
});

test("dead keys: ' then r types 'r once, ' then a types á (ABNT2 / US-International)", async ({ page }) => {
  await page.goto("/");
  await addProject(page);
  await page.keyboard.press("Meta+n");
  const ta = page.locator(".term-host .xterm-helper-textarea").first();
  await expect(ta).toBeAttached();
  await ta.focus();
  const typed = () => page.evaluate(() => (window as any).__fleetMock.ptyWrites.map((w: any) => w.data).join(""));
  const start = (await typed()).length;
  await page.keyboard.type("we");
  // What macOS WebKit fires for a dead key, then a key it can't accent (two orders seen in the
  // wild) or one it can.
  await page.evaluate(() => {
    const t = document.activeElement as HTMLTextAreaElement;
    const key = (k: string, composing: boolean) => {
      const e = new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, isComposing: composing });
      Object.defineProperty(e, "keyCode", { value: 229 });
      t.dispatchEvent(e);
    };
    const comp = (type: string, data: string) => t.dispatchEvent(new CompositionEvent(type, { data, bubbles: true }));
    const input = (inputType: string, data: string) => t.dispatchEvent(new InputEvent("input", { inputType, data, bubbles: true }));
    const dead = () => { key("Dead", false); comp("compositionstart", ""); comp("compositionupdate", "'"); t.value += "'"; input("insertCompositionText", "'"); };
    // ' r → commit "'" then insert "r"
    dead(); key("r", true); comp("compositionend", "'"); t.value += "r"; input("insertText", "r");
    // ' r → commit "'r" at once
    dead(); key("r", true); comp("compositionupdate", "'r"); input("insertCompositionText", "'r"); comp("compositionend", "'r");
    // ' a → á
    dead(); key("a", true); comp("compositionupdate", "á"); input("insertCompositionText", "á"); comp("compositionend", "á");
  });
  await page.keyboard.type("e");
  await expect.poll(async () => (await typed()).slice(start)).toBe("we'r'ráe");
});
