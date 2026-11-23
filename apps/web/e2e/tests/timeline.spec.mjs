import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";

const BASE = "/app/list/01930000-0000-7000-8002-000000000028";
const ROOM = `${BASE}?fixture=timeline`;
const PLANNED = "/app/planned?fixture=timeline";
const TITLE = "Strip wallpaper and prime hallway walls";
const GROUT = "Pick grout color for bathroom tiles";
const LIGHTS = "Compare kitchen pendant lights";
const PANELS = "Research acoustic panels for study";
const INVOICE = "Review the budget";
const DESKTOP = { width: 1440, height: 900 };
const PHONE = { width: 412, height: 920 };
const zoom = (p, name) => p.getByRole("group", { name: "Timeline zoom" }).getByRole("button", { name, exact: true });
const tab = (p, name) => p.getByRole("tab", { name, exact: true });
const rail = (p, name) => p.locator(".timeline-task").getByText(name, { exact: true });
const lane = (p, name) => p.locator(".timeline-lane", { has: rail(p, name) });
const bar = (p, name) => lane(p, name).locator(".timeline-bar");
const row = (p, name) => p.getByRole("button", { name, exact: true }).and(p.locator(".timeline-task"));
const day = (p, date) => p.locator(`.timeline-ruler [data-date="${date}"]`);
const undo = (p) => p.locator(".toast-undo");
const tray = (p) => p.getByRole("button", { name: /^Unscheduled tasks,/ });
const timeline = (p) => p.locator(".timeline-scroll");

async function shiftPointer(page, from, dx, dy = 0, release = true) {
  const { x, y, width, height } = await from.boundingBox();
  const sx = x + width / 2;
  const sy = y + height / 2;
  await page.mouse.move(sx, sy);
  await page.mouse.down();
  await page.mouse.move(sx + dx, sy + dy, { steps: 8 });
  if (release) await page.mouse.up();
}

async function dragTo(page, from, to) {
  const a = await from.boundingBox();
  const b = await to.boundingBox();
  await shiftPointer(page, from, b.x + b.width / 2 - a.x - a.width / 2, b.y + b.height / 2 - a.y - a.height / 2);
}

async function visibleDates(page) {
  return timeline(page).evaluate((scroll) => {
    const rail = scroll.querySelector("#timeline-rail-toggle").getBoundingClientRect();
    const end = scroll.getBoundingClientRect().left + scroll.clientWidth;
    return [...scroll.querySelectorAll(".timeline-ruler [data-date]")].filter((cell) => {
      const r = cell.getBoundingClientRect();
      return r.right > rail.right + 1 && r.left < end - 1;
    }).map((cell) => cell.dataset.date);
  });
}

async function expectDates(taskBar, start, due) {
  if (start) await expect(taskBar).toHaveAttribute("data-start", start);
  else await expect(taskBar).not.toHaveAttribute("data-start", /.+/);
  if (due) await expect(taskBar).toHaveAttribute("data-due", due);
  else await expect(taskBar).not.toHaveAttribute("data-due", /.+/);
}

test.use({ reducedMotion: "reduce" });
test.beforeEach(async ({ page }) => page.setViewportSize(DESKTOP));

test("four scales keep the centre, Today jumps back, and zoom is remembered", async ({ page }) => {
  const w = watch(page);
  await open(page, ROOM);
  await expect(tab(page, "Timeline")).toHaveAttribute("aria-selected", "true");
  await expect(zoom(page, "Two weeks")).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator(".timeline-range")).toHaveText(/19\.11\.–02\.12\./);
  expect(await visibleDates(page)).toHaveLength(14);
  await expect(day(page, "2026-11-21")).toHaveAttribute("aria-current", "date");
  await expect(page.locator(".timeline-today-line")).toBeInViewport();
  await expect(page.locator(".timeline-today-dot")).toBeInViewport();
  for (const [name, min, max] of [["Week", 7, 8], ["Month", 35, 36], ["Quarter", 12, 13], ["Two weeks", 14, 15]]) {
    await zoom(page, name).click();
    await expect(zoom(page, name)).toHaveAttribute("aria-pressed", "true");
    const dates = await visibleDates(page);
    expect(dates.length).toBeGreaterThanOrEqual(min);
    expect(dates.length).toBeLessThanOrEqual(max);
    expect(dates[0] <= "2026-11-26" && dates.at(-1) >= "2026-11-26").toBe(true);
  }
  await page.getByRole("button", { name: "Next dates" }).click();
  await expect(page.locator(".timeline-range")).toHaveText(/03\.–16\.12\./);
  await page.locator("#room-title").focus();
  await page.keyboard.press("0");
  await expect(page.locator(".timeline-range")).toHaveText(/19\.11\.–02\.12\./);
  await zoom(page, "Month").click();
  await page.reload();
  await expect(zoom(page, "Month")).toHaveAttribute("aria-pressed", "true");
  await w.check();
});

test("bar moves and resizes snap to dates and leave one exact Undo", async ({ page }) => {
  const w = watch(page);
  await open(page, ROOM);
  const span = bar(page, TITLE);
  const milestone = bar(page, GROUT);
  const width = (await day(page, "2026-11-21").boundingBox()).width;
  await shiftPointer(page, span, width * 2, 0, false);
  await expect(page.locator(".timeline-drag-readout")).toContainText("21.–25.11.");
  await expectDates(span, "2026-11-19", "2026-11-23");
  await page.mouse.up();
  await expectDates(span, "2026-11-21", "2026-11-25");
  await expect(page.locator(".toast")).toContainText("Rescheduled");
  await expect(page.locator("#task-detail")).not.toBeVisible();
  await undo(page).click();
  await expectDates(span, "2026-11-19", "2026-11-23");

  await shiftPointer(page, span.locator(".timeline-resize.due"), width);
  await expectDates(span, "2026-11-19", "2026-11-24");
  await expect(row(page, TITLE)).toBeFocused();
  await page.keyboard.press("Control+z");
  await expectDates(span, "2026-11-19", "2026-11-23");

  await expect(milestone).toHaveClass(/milestone/);
  await shiftPointer(page, milestone.locator(".timeline-resize.start"), -width);
  await expectDates(milestone, "2026-11-19", "2026-11-20");
  await expect(milestone).not.toHaveClass(/milestone/);
  await undo(page).click();
  await expectDates(milestone, null, "2026-11-20");

  await shiftPointer(page, span, width, 0, false);
  await expect(page.locator(".timeline-drag-readout")).toBeVisible();
  await page.keyboard.press("Escape");
  await page.mouse.up();
  await expectDates(span, "2026-11-19", "2026-11-23");
  await expect(page.locator(".timeline-drag-readout")).toBeHidden();
  await expect(page.locator("#task-detail")).not.toBeVisible();
  await w.check();
});

test("keyboard selects lanes, reschedules by day and week, resizes, and opens both date pickers", async ({ page }) => {
  const w = watch(page);
  await open(page, ROOM);
  await page.locator("#room-title").focus();
  await page.keyboard.press("J");
  await expect(row(page, TITLE)).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(row(page, GROUT)).toBeFocused();
  await page.keyboard.press("K");
  await expect(row(page, TITLE)).toBeFocused();
  const span = bar(page, TITLE);
  await page.keyboard.press("Control+Alt+ArrowRight");
  await expectDates(span, "2026-11-20", "2026-11-24");
  await page.keyboard.press("Shift+Alt+ArrowRight");
  await expectDates(span, "2026-11-27", "2026-12-01");
  await page.keyboard.press("[");
  await expectDates(span, "2026-11-27", "2026-11-30");
  await page.keyboard.press("]");
  await expectDates(span, "2026-11-27", "2026-12-01");
  // Undo queues a focus return for the next frame. Open Due in that same frame, before
  // the queued return runs, to exercise fast keyboard input even on a slow browser.
  await page.evaluate(() => {
    for (const keys of [{ key: "z", ctrlKey: true }, { key: "d" }]) {
      document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { ...keys, bubbles: true, cancelable: true }));
    }
    return new Promise((resolve) => requestAnimationFrame(() => resolve()));
  });
  await expectDates(span, "2026-11-27", "2026-11-30");
  await expect(page.locator("#task-detail")).not.toBeVisible();
  const due = page.getByRole("dialog", { name: "Choose due date" });
  await expect(due).toBeVisible();
  await expect(due.locator("#timeline-active-day")).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(due).toBeHidden();
  await expect(row(page, TITLE)).toBeFocused();
  await page.keyboard.press("Shift+D");
  await page.getByRole("dialog", { name: "Choose start date" }).getByRole("button", { name: "Tomorrow", exact: true }).click();
  await expectDates(span, "2026-11-22", "2026-11-30");
  await expect(row(page, TITLE)).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.locator("#task-detail")).toBeVisible();
  await w.check();
});

test("tray Schedule and drag assign due dates; undo restores the tray; completed lanes toggle", async ({ page }) => {
  const w = watch(page);
  await open(page, ROOM);
  const lights = page.getByRole("button", { name: `Schedule ${LIGHTS}`, exact: true });
  await expect(tray(page)).toHaveAttribute("aria-expanded", "false");
  await tray(page).click();
  await lights.click();
  await page.getByRole("dialog", { name: "Choose due date" }).getByRole("button", { name: "Tomorrow", exact: true }).click();
  await expectDates(bar(page, LIGHTS), null, "2026-11-22");
  await expect(lights).toHaveCount(0);
  await expect(row(page, LIGHTS)).toBeFocused();
  await undo(page).click();
  await expect(lights).toBeVisible();
  await expect(bar(page, LIGHTS)).toHaveCount(0);
  const panels = page.locator(".timeline-tray-card", { hasText: PANELS });
  await dragTo(page, panels.locator(".timeline-grip"), day(page, "2026-11-24"));
  await expectDates(bar(page, PANELS), null, "2026-11-24");
  await undo(page).click();
  await expect(panels).toBeVisible();
  const toggle = page.locator(".completed-toggle");
  const complete = lane(page, "Deliver old sofa to charity depot");
  await expect(complete).toHaveCount(0);
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-expanded", "true");
  await expect(complete).toBeVisible();
  await toggle.click();
  await expect(complete).toHaveCount(0);
  await row(page, TITLE).focus();
  await page.keyboard.press("Space");
  await expect(lane(page, TITLE)).toHaveClass(/completing/);
  await expect(lane(page, TITLE)).toHaveCount(0);
  await expect(row(page, GROUT)).toBeFocused();
  await undo(page).click();
  await expect(lane(page, TITLE)).toBeVisible();
  await w.check();
});

test("planned and eligible collections use list tints; a list can save Timeline as its default", async ({ page }) => {
  const w = watch(page);
  await open(page, BASE + "?fixture=workflow");
  await expect(tab(page, "Board")).toHaveAttribute("aria-selected", "true");
  await page.getByRole("button", { name: "Room menu", exact: true }).click();
  await page.getByRole("menuitem", { name: "List settings…", exact: true }).click();
  await page.getByRole("group", { name: "Default view" }).getByRole("button", { name: "Timeline", exact: true }).click();
  await expect(tab(page, "Board")).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  const nav = page.locator(".sidebar");
  await nav.getByRole("link", { name: /^Home \d/ }).click();
  await expect(tab(page, "Board")).toBeHidden();
  await tab(page, "Timeline").click();
  await expect(lane(page, TITLE)).toHaveClass(/cross-list/);
  await expect(lane(page, TITLE)).toHaveAttribute("data-list-hue", "clay");
  await nav.getByRole("link", { name: /^Home renovation/ }).click();
  await expect(tab(page, "Timeline")).toHaveAttribute("aria-selected", "true");
  await open(page, PLANNED);
  await tab(page, "Timeline").click();
  await expect(bar(page, INVOICE)).toBeVisible();
  await expect(lane(page, INVOICE)).toHaveAttribute("data-list-hue", "cornflower");
  const startOnly = bar(page, "Draft the announcement");
  await expect(startOnly).toHaveClass(/open-ended/);
  await expectDates(startOnly, "2026-11-30", null);
  await expect(page.locator(".timeline-tray")).toHaveCount(0);
  await page.getByRole("button", { name: "Filter tasks", exact: true }).click();
  await page.getByRole("searchbox", { name: "Filter tasks" }).fill("start:any due:none");
  await expect(page.locator(".timeline-lane")).toHaveCount(1);
  await expect(startOnly).toBeVisible();
  await w.check();
});

test("narrow room compacts controls and the sticky rail, scrolls locally and opens modal detail", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize(PHONE);
  await open(page, ROOM);
  await expect(tab(page, "Timeline").getByText("Timeline", { exact: true })).toBeHidden();
  await expect(zoom(page, "Week").getByText("1W", { exact: true })).toBeVisible();
  const railBox = await page.locator("#timeline-rail-toggle").boundingBox();
  expect(railBox.width).toBeLessThan(224);
  await expect(page.locator(".timeline-today-dot")).toBeInViewport();
  const left = (await row(page, TITLE).boundingBox()).x;
  const old = await timeline(page).evaluate((el) => el.scrollLeft);
  await timeline(page).evaluate((el) => el.scrollLeft += 30);
  await expect.poll(() => timeline(page).evaluate((el) => el.scrollLeft)).toBeGreaterThan(old);
  expect((await row(page, TITLE).boundingBox()).x).toBe(left);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(PHONE.width);
  await page.getByRole("button", { name: "Collapse task rail" }).click();
  await expect(page.locator("#timeline-rail-toggle")).toHaveCSS("width", "44px");
  await page.getByRole("button", { name: "Expand task rail" }).click();
  await row(page, TITLE).click();
  const detail = page.getByRole("dialog", { name: "Task detail" });
  await expect(detail).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(detail).not.toBeVisible();
  await expect(row(page, TITLE)).toBeFocused();
  await tray(page).click();
  await expect(page.getByRole("button", { name: `Schedule ${LIGHTS}`, exact: true })).toBeInViewport();
  await w.check();
});

for (const scheme of ["light", "dark"]) {
  test.describe(`Timeline gallery in ${scheme}`, () => {
    test.use({ colorScheme: scheme });
    test("2W, Month, tray, Planned and narrow", async ({ page }) => {
      const w = watch(page);
      const shot = (name) => page.screenshot({ path: `gallery/timeline-${name}-${scheme}.png`, animations: "disabled" });
      await open(page, ROOM);
      await expect(bar(page, TITLE)).toBeInViewport();
      await row(page, "Order matte oak flooring samples").focus();
      await shot("2w");
      await page.keyboard.press("Escape");
      await zoom(page, "Month").click();
      await page.getByRole("button", { name: "Today", exact: true }).click();
      await expect(bar(page, TITLE)).toBeInViewport();
      await shot("month");
      await zoom(page, "Two weeks").click();
      await page.getByRole("button", { name: "Today", exact: true }).click();
      await tray(page).click();
      await expect(page.getByRole("button", { name: `Schedule ${LIGHTS}`, exact: true })).toBeInViewport();
      await shot("tray");
      await tray(page).click();
      await open(page, PLANNED);
      await tab(page, "Timeline").click();
      await expect(bar(page, INVOICE)).toBeInViewport();
      await shot("planned");
      await page.setViewportSize(PHONE);
      await open(page, ROOM);
      await expect(bar(page, TITLE)).toBeInViewport();
      await shot("narrow");
      await w.check();
    });
  });
}
