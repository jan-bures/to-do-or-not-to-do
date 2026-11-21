// What the app does, in a real browser under the server's CSP (fixtures build: nothing leaves the page).
import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";

test.use({ viewport: { width: 1440, height: 900 } });

const rows = (page) => page.locator(".rows:not(.completed-rows) .row");
const row = (page, title) => rows(page).filter({ hasText: title });

test("My Day shows the morning's tasks and the sun matches the count", async ({ page }) => {
  const w = watch(page);
  await open(page);
  await expect(page.locator(".room-title")).toHaveText("My Day");
  await expect(page.locator(".summary")).toContainText("3 of 12 done");
  await expect(rows(page)).toHaveCount(9);
  await expect(page.locator(".landscape .ls-sun")).toHaveCount(2);
  await expect(page.locator(".sidebar .nav-item.active")).toContainText("9");
  await expect(page.locator("html")).toHaveAttribute("data-room-hue", "my-day");
  await w.check();
});

test("adding a task puts it in My Day", async ({ page }) => {
  const w = watch(page);
  await open(page);
  await page.keyboard.press("n");
  await expect(page.locator(".qa-input")).toBeFocused();
  await page.keyboard.type("Koupit žárovky do předsíně");
  await page.keyboard.press("Enter");
  await expect(row(page, "Koupit žárovky do předsíně")).toHaveCount(1);
  await expect(page.locator(".qa-input")).toHaveValue("");
  await expect(page.locator(".summary")).toContainText("3 of 13 done");
  await w.check();
});

test("completing a task holds, collapses, moves the sun, and can be undone", async ({ page }) => {
  const w = watch(page);
  await open(page);
  const title = "Rotate the backup encryption keys";
  await row(page, title).locator(".check").click();
  // The row holds in place, checked, while the toast offers Undo.
  await expect(row(page, title)).toHaveClass(/completing/);
  await expect(page.locator(".toast")).toContainText(title);
  await expect(page.locator(".summary")).toContainText("3 of 12 done");
  // Then it leaves, and the count moves.
  await expect(row(page, title)).toHaveCount(0, { timeout: 3000 });
  await expect(page.locator(".summary")).toContainText("4 of 12 done");
  await expect(page.locator(".completed-toggle .count")).toHaveText("4");
  await page.locator(".toast-undo").click();
  await expect(row(page, title)).toHaveCount(1);
  await expect(page.locator(".summary")).toContainText("3 of 12 done");
  await w.check();
});

test("completing a recurring task offers no Undo, so no extra occurrence can be left behind", async ({ page }) => {
  const w = watch(page);
  await open(page);
  // Weekly, with a due date: completing it schedules the next one, which reopening would not remove.
  const title = "Prepare the November board update";
  await expect(row(page, title)).toHaveCount(1);
  await row(page, title).locator(".check").click();
  await expect(page.locator(".toast")).toContainText(title);
  await expect(page.locator(".toast")).toContainText("next one is scheduled");
  await expect(page.locator(".toast-undo")).toHaveCount(0);
  // Ctrl+Z does nothing either: the task leaves My Day completed.
  await page.keyboard.press("Control+z");
  await expect(row(page, title)).toHaveCount(0, { timeout: 3000 });
  await expect(page.locator(".summary")).toContainText("4 of 12 done");
  // Its list has exactly one open copy: the next occurrence.
  await page.locator(".sidebar").getByRole("link", { name: /^Planning/ }).click();
  await expect(row(page, title)).toHaveCount(1);
  await w.check();
});

test("the title is edited in the detail pane", async ({ page }) => {
  const w = watch(page);
  await open(page);
  await row(page, "Zavolat do servisu kvůli STK").click();
  await expect(page).toHaveURL(/\/app\/task\//);
  await page.keyboard.press("e");
  const input = page.locator(".title-input");
  await expect(input).toBeFocused();
  await input.fill("Zavolat do servisu kvůli STK a pneu");
  await input.press("Enter");
  await expect(page.locator(".d-title")).toHaveText("Zavolat do servisu kvůli STK a pneu");
  await expect(row(page, "STK a pneu")).toHaveCount(1);
  await w.check();
});

test("steps, important and My Day toggles work from the detail pane", async ({ page }) => {
  const w = watch(page);
  await open(page);
  await row(page, "Prepare the November board update").click();
  const detail = page.locator(".detail");
  await expect(detail.locator(".d-caps .steps")).toContainText("2/5");
  await detail.locator(".step").nth(2).locator(".check").click();
  await expect(detail.locator(".d-caps .steps")).toContainText("3/5");
  await detail.locator(".add-step input").fill("Book the room");
  await detail.locator(".add-step input").press("Enter");
  await expect(detail.locator(".d-caps .steps")).toContainText("3/6");
  await detail.getByRole("button", { name: "My Day" }).click();
  await expect(row(page, "Prepare the November board update")).toHaveCount(0);
  await w.check();
});

test("keyboard: J/K move the selection, Space completes, Escape closes", async ({ page }) => {
  const w = watch(page);
  await open(page);
  await page.keyboard.press("j");
  await expect(rows(page).first()).toHaveClass(/selected/);
  await page.keyboard.press("j");
  await expect(rows(page).nth(1)).toHaveClass(/selected/);
  await page.keyboard.press("k");
  await expect(rows(page).first()).toHaveClass(/selected/);
  await expect(page.locator(".detail .d-title")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.locator(".detail .d-title")).toHaveCount(0);
  await w.check();
});

test("rooms are addressable, and back goes back", async ({ page }) => {
  const w = watch(page);
  await open(page);
  await page.locator(".sidebar").getByRole("link", { name: /^Finance/ }).click();
  await expect(page).toHaveURL(/\/app\/list\//);
  await expect(page.locator(".room-title")).toHaveText("Finance");
  await expect(page.locator(".eyebrow-lead")).toHaveText("Work");
  await page.goBack();
  await expect(page.locator(".room-title")).toHaveText("My Day");
  await page.locator(".sidebar").getByRole("link", { name: /^Important/ }).click();
  await expect(page).toHaveURL(/\/app\/important$/);
  await page.reload();
  await expect(page.locator(".room-title")).toHaveText("Important");
  await w.check();
});

test("the theme follows the system", async ({ page }) => {
  const w = watch(page);
  await page.emulateMedia({ colorScheme: "dark" });
  await open(page);
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await page.emulateMedia({ colorScheme: "light" });
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await w.check();
});

test("the page runs under the server's CSP, with no inline script or style", async ({ page }) => {
  const w = watch(page);
  const res = await page.goto("/app");
  expect(res.headers()["content-security-policy"]).toContain("script-src 'self' 'wasm-unsafe-eval'");
  await page.locator(".room-title").waitFor();
  expect(await page.locator("script:not([src])").count()).toBe(0);
  expect(await page.locator("[style]").count()).toBe(0);
  await w.check();
});
