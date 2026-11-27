import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";

const DESKTOP = { width: 1440, height: 900 };
const SHORT = { width: 1440, height: 520 };
const PHONE = { width: 412, height: 920 };

test("manual header toggle collapses to a compact bar, persists across reload, and works across views", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize(DESKTOP);
  await open(page);

  const head = page.locator(".room-head");
  const toggle = page.getByRole("button", { name: "Collapse header", exact: true });
  await expect(toggle).toBeVisible();
  const expandedHeight = (await head.boundingBox()).height;
  expect(expandedHeight).toBeGreaterThan(120);

  await toggle.click();
  const expand = page.getByRole("button", { name: "Expand header", exact: true });
  await expect(expand).toBeVisible();
  await expect(head).toHaveClass(/\bcollapsed\b/);
  const collapsedHeight = (await head.boundingBox()).height;
  expect(collapsedHeight).toBeLessThan(60);
  expect(expandedHeight - collapsedHeight).toBeGreaterThan(80);
  await expect(page.locator(".room-head .compact-progress")).toBeVisible();

  // Collapsed header matches the detail top bar height, and row checkboxes center on the row.
  await page.locator(".row .row-title").first().click();
  await expect(page.locator(".detail-top")).toBeVisible();
  const [headBox, detailTopBox, checkBox, gripBox] = await Promise.all([
    head.boundingBox(),
    page.locator(".detail-top").boundingBox(),
    page.locator(".row > .check").first().boundingBox(),
    page.locator(".row .task-drag-grip").first().boundingBox(),
  ]);
  expect(Math.abs(headBox.height - detailTopBox.height)).toBeLessThanOrEqual(0.5);
  expect(Math.abs((checkBox.y + checkBox.height / 2) - (gripBox.y + gripBox.height / 2))).toBeLessThanOrEqual(1);

  // Persists across rooms and reloads.
  await page.locator(".sidebar").getByRole("link", { name: /^Work/ }).click();
  await expect(page.locator(".room-head")).toHaveClass(/\bcollapsed\b/);
  await page.reload();
  await expect(page.locator(".room-title")).toHaveText("Work");
  await expect(page.locator(".room-head")).toHaveClass(/\bcollapsed\b/);

  // Works on Board and Timeline views too.
  await open(page, "/app/list/01930000-0000-7000-8002-000000000028?fixture=workflow");
  await expect(page.locator(".room-head")).toHaveClass(/\bcollapsed\b/);
  await page.getByRole("tab", { name: "Timeline", exact: true }).click();
  await expect(page.locator(".room-head")).toHaveClass(/\bcollapsed\b/);
  await page.getByRole("tab", { name: "List", exact: true }).click();
  await expect(page.locator(".room-head")).toHaveClass(/\bcollapsed\b/);

  // Expanding restores the full hero and clears the persisted preference.
  await page.getByRole("button", { name: "Expand header", exact: true }).click();
  await expect(page.locator(".room-head")).not.toHaveClass(/\bcollapsed\b/);
  await page.reload();
  await expect(page.locator(".room-head")).not.toHaveClass(/\bcollapsed\b/);
  await w.check();
});

test("scrolling past the hero docks the compact bar and clicking expand returns to the top", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize(SHORT);
  await open(page);

  const room = page.locator(".room");
  const bar = page.locator(".room-compact-bar");
  await expect(bar).not.toHaveClass(/\bvisible\b/);

  const scrolled = await room.evaluate((el) => {
    el.scrollTop = el.scrollHeight;
    return el.scrollTop;
  });
  expect(scrolled).toBeGreaterThan(120);
  await expect(bar).toHaveClass(/\bvisible\b/);
  await expect(bar.locator(".compact-bar-title")).toHaveText("My Day");
  await expect(bar.locator(".compact-progress")).toBeVisible();

  await bar.getByRole("button", { name: "Expand header", exact: true }).click();
  await expect.poll(() => room.evaluate((el) => el.scrollTop)).toBe(0);
  await expect(bar).not.toHaveClass(/\bvisible\b/);
  await w.check();
});

test("collapsed header works on finished day and phone viewports", async ({ page }) => {
  const w = watch(page);
  for (const scheme of ["light", "dark"]) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.setViewportSize(DESKTOP);
    await open(page);
    await page.getByRole("button", { name: "Collapse header", exact: true }).click();
    await expect(page.locator(".room-head")).toHaveClass(/\bcollapsed\b/);
    await page.locator(".row .row-title").first().click();
    await expect(page.locator(".detail-top")).toBeVisible();
    await page.screenshot({ path: `gallery/desktop-header-collapsed-${scheme}.png`, animations: "disabled" });
    await page.getByRole("button", { name: "Expand header", exact: true }).click();
  }

  await page.setViewportSize(DESKTOP);
  await open(page, "/app?fixture=finished");
  await expect(page.locator(".room-head.finished")).toBeVisible();
  await page.getByRole("button", { name: "Collapse header", exact: true }).click();
  await expect(page.locator(".room-head.finished")).toHaveClass(/\bcollapsed\b/);
  expect((await page.locator(".room-head.finished").boundingBox()).height).toBeLessThan(60);
  await page.getByRole("button", { name: "Expand header", exact: true }).click();

  await page.setViewportSize(PHONE);
  await open(page);
  await page.getByRole("button", { name: "Collapse header", exact: true }).click();
  await expect(page.locator(".room-head")).toHaveClass(/\bcollapsed\b/);
  expect((await page.locator(".room-head").boundingBox()).height).toBeLessThan(60);
  await w.check();
});
