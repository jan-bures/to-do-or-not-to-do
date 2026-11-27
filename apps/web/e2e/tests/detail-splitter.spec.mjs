import { readFileSync } from "node:fs";
import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";

const { metrics: { desktop }, elevation } = JSON.parse(readFileSync(
  new URL("../../../../crates/todo-design/generated/tokens.json", import.meta.url), "utf8",
));
const WIDE = { width: 1920, height: 1080 };
const DESKTOP = { width: 1440, height: 900 };
const min = desktop["detail-min"];
const initial = desktop.detail;
const max = (width) => width - desktop.sidebar - desktop["list-min-beside-detail"] - 2 * elevation.hairline;
const pane = (page) => page.getByRole("region", { name: "Task detail", exact: true });
const splitter = (page) => page.getByRole("separator", { name: "Task detail width" });

async function showDetail(page) {
  await page.getByRole("button", { name: "Prepare the November board update", exact: true }).click();
  return splitter(page);
}

async function expectWidth(page, width) {
  await expect(splitter(page)).toHaveAttribute("aria-valuenow", String(width));
  await expect(pane(page)).toHaveCSS("width", `${width}px`);
}

test("the docked separator is tabbable and resizes the detail by keyboard", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize(DESKTOP);
  await open(page);
  await expect(splitter(page)).toHaveCount(0);
  const handle = await showDetail(page);
  await expect(handle).toHaveAttribute("aria-orientation", "vertical");
  await expect(handle).toHaveAttribute("aria-controls", "task-detail");
  await expect(handle).toHaveAttribute("aria-valuemin", String(min));
  await expect(handle).toHaveAttribute("aria-valuemax", String(max(DESKTOP.width)));
  await expectWidth(page, initial);
  await pane(page).getByRole("button", { name: "My Day", exact: true }).focus();
  await page.keyboard.press("Shift+Tab");
  await expect(handle).toBeFocused();
  const taskCheck = page.getByRole("main").getByRole("checkbox", { name: /November board update/ });
  await expect(taskCheck).not.toBeChecked();
  await page.keyboard.press("Space");
  await expect(taskCheck).not.toBeChecked();
  await expect(page.locator(".toast-text").filter({ hasText: /Completed.*November board update/ })).toHaveCount(0);
  await page.keyboard.press("ArrowLeft");
  await expectWidth(page, initial + desktop["pane-key-step"]);
  await page.keyboard.press("Shift+ArrowRight");
  await expectWidth(page, initial + desktop["pane-key-step"] - desktop["pane-key-step-large"]);
  await page.keyboard.press("End");
  await expectWidth(page, max(DESKTOP.width));
  await page.keyboard.press("Shift+ArrowRight");
  await expectWidth(page, max(DESKTOP.width) - desktop["pane-key-step-large"]);
  await page.keyboard.press("Home");
  await expectWidth(page, initial);
  for (let i = 0; i < 4; i++) await page.keyboard.press("Shift+ArrowRight");
  await expectWidth(page, min);
  await page.keyboard.press("Escape");
  await expect(page.getByRole("main").getByRole("button", { name: "Prepare the November board update", exact: true })).toBeFocused();
  await expectWidth(page, min);
  await page.reload();
  await expectWidth(page, min);
  await w.check();
});

test("the separator shows keyboard focus in forced colours", async ({ page }) => {
  const w = watch(page);
  await page.emulateMedia({ forcedColors: "active" });
  await page.setViewportSize(DESKTOP);
  await open(page);
  const handle = await showDetail(page);
  await expect(handle).toHaveCSS("outline-style", "none");
  await pane(page).getByRole("button", { name: "My Day", exact: true }).focus();
  await page.keyboard.press("Shift+Tab");
  await expect(handle).toBeFocused();
  await expect(handle).toHaveCSS("outline-style", "solid");
  await w.check();
});

test("drag captures the pointer, tracks without lag and saves on release", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize(DESKTOP);
  await open(page);
  const handle = await showDetail(page);
  await expect(handle).toHaveCSS("cursor", "col-resize");
  await expect(handle).toHaveCSS("width", `${desktop["pane-handle-hit"]}px`);
  await expectWidth(page, initial);
  const { x } = await handle.boundingBox();
  await page.mouse.move(x + 2, 60);
  await expect.poll(() => handle.evaluate((el) => getComputedStyle(el, "::after").opacity)).toBe("1");
  await page.mouse.down();
  // Leave the strip (and the pane) vertically while dragging across the list.
  await page.mouse.move(x - 178, 890);
  await expectWidth(page, initial + 180);
  await expect(handle).toHaveClass(/\bgrabbing\b/);
  await expect.poll(() => handle.evaluate((el) => getComputedStyle(el, "::after").opacity)).toBe("1");
  await page.mouse.move(100, 890);
  await expectWidth(page, max(DESKTOP.width));
  await page.mouse.move(1410, 890);
  await expectWidth(page, min);
  await page.mouse.move(x - 198, 890);
  await expectWidth(page, initial + 200);
  await page.mouse.up();
  await expect(handle).not.toHaveClass(/\bgrabbing\b/);
  await expect.poll(() => handle.evaluate((el) => getComputedStyle(el, "::after").opacity)).toBe("0");
  await page.reload();
  await expectWidth(page, initial + 200);
  await handle.dblclick();
  await expectWidth(page, initial);

  const resetX = (await handle.boundingBox()).x + 2;
  await page.mouse.move(resetX, 60);
  await page.mouse.down();
  await page.mouse.move(resetX - 180, 890);
  await expectWidth(page, initial + 180);
  await page.keyboard.press("Escape");
  await expectWidth(page, initial);
  await expect(handle).not.toHaveClass(/\bgrabbing\b/);
  await page.mouse.up();
  await w.check();
});

test("a viewport width change cancels a drag without saving its preview", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize(WIDE);
  await open(page);
  const handle = await showDetail(page);
  await handle.press("End");
  await expectWidth(page, max(WIDE.width));
  const x = (await handle.boundingBox()).x + 2;
  await page.mouse.move(x, 60);
  await page.mouse.down();
  await page.mouse.move(x + 120, 60);
  await expectWidth(page, max(WIDE.width) - 120);
  await expect(handle).toHaveClass(/\bgrabbing\b/);

  await page.setViewportSize(DESKTOP);
  await expect(handle).not.toHaveClass(/\bgrabbing\b/);
  await expectWidth(page, max(DESKTOP.width));
  await page.mouse.move(x + 160, 60);
  await page.mouse.up();
  await page.setViewportSize(WIDE);
  await expectWidth(page, max(WIDE.width));
  await page.reload();
  await expectWidth(page, max(WIDE.width));
  await w.check();
});

test("viewport and reload clamp the live width without replacing the saved preference", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize(WIDE);
  await open(page);
  const handle = await showDetail(page);
  await handle.press("End");
  await expectWidth(page, max(WIDE.width));
  await page.setViewportSize(DESKTOP);
  await expect(handle).toHaveAttribute("aria-valuemax", String(max(DESKTOP.width)));
  await expectWidth(page, max(DESKTOP.width));
  // A key press at a clamped limit is not a new preference.
  await handle.press("ArrowLeft");
  await page.reload();
  await expectWidth(page, max(DESKTOP.width));
  await page.setViewportSize({ width: 1199, height: 900 });
  await expect(handle).toHaveCount(0);
  await expect(page.getByRole("dialog", { name: "Task detail" })).toHaveCSS("width", `${initial}px`);
  await page.setViewportSize(WIDE);
  await expect(handle).toHaveAttribute("aria-valuemax", String(max(WIDE.width)));
  await expectWidth(page, max(WIDE.width));
  await w.check();
});

test("a squeezed list stacks counts and aligns metadata under the title; wide prose stays left-aligned", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize(WIDE);
  await open(page, "/app?fixture=detail-wide");
  const title = page.getByRole("button", { name: /^Prepare the quarterly roadmap review/ });
  await title.click();
  const row = page.locator(".row").filter({ has: page.locator(".row-title", { hasText: /^Prepare the quarterly roadmap review/ }) });
  const meta = row.locator(".meta");
  const step = pane(page).getByRole("button", { name: /^Check the proposed milestones/ });
  await expect(step.locator("span")).toHaveCSS("white-space", "normal");
  await expect(page.locator(".title-row")).toHaveCSS("flex-direction", "row");
  expect((await meta.boundingBox()).width).toBeCloseTo((await row.locator(".row-title").boundingBox()).width);

  await splitter(page).press("End");
  await expectWidth(page, max(WIDE.width));
  const main = await page.getByRole("main").boundingBox();
  expect(main.width - elevation.hairline).toBe(desktop["list-min-beside-detail"]);
  await expect(page.locator(".title-row")).toHaveCSS("flex-direction", "column");
  const titleBox = await row.locator(".row-title").boundingBox();
  const metaBox = await meta.boundingBox();
  expect(metaBox.x).toBeCloseTo(titleBox.x);
  expect(metaBox.width).toBeGreaterThan(titleBox.width);
  expect(metaBox.y).toBeGreaterThan(titleBox.y);
  expect((await row.locator(".col-due").boundingBox()).y).toBeCloseTo(titleBox.y);

  const text = pane(page).locator(".notes");
  await expect(step.locator("span")).toHaveCSS("white-space", "normal");
  const fields = await pane(page).locator(".props").boundingBox();
  for (const prose of [text, pane(page).locator(".d-title"), step]) {
    const measure = await prose.evaluate((el) => [
      getComputedStyle(el).getPropertyValue("--desktop-detail-text-measure").trim(),
      parseFloat(getComputedStyle(el).maxInlineSize),
    ]);
    const box = await prose.boundingBox();
    expect(measure[0]).toBe(`${desktop["detail-text-measure"]}ch`);
    expect(box.width).toBeCloseTo(measure[1], 0);
    expect(box.width).toBeLessThan(fields.width);
  }
  expect((await text.boundingBox()).x).toBeCloseTo(fields.x);
  await w.check();
});
