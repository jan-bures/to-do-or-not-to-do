import { expect, test } from "@playwright/test";
import { dragOver, open, watch } from "./helpers.mjs";

const nav = (page) => page.locator("#room-navigation");
const room = (page, name) => nav(page).getByRole("link", { name: new RegExp(`^${name}( |$)`) });
const group = (page, name) => nav(page).locator(".nav-group").filter({
  has: page.getByRole("link", { name: new RegExp(`^${name}( |$)`) }),
});
const names = (scope) => scope.locator(".nav-name");

async function pickup(page, link) {
  await link.scrollIntoViewIfNeeded();
  const { x, y, width, height } = await link.boundingBox();
  await page.mouse.move(x + width / 3, y + height / 2);
  await page.mouse.down();
  await page.mouse.move(x + width / 3, y + height / 2 + 8);
  const lift = page.locator(".sidebar-drag-lift");
  await expect(lift).toBeVisible();
  const bounds = await lift.boundingBox();
  expect(bounds.x).toBeCloseTo(x, 1);
  expect(bounds.y).toBeCloseTo(y + 8, 1);
  expect(bounds.width).toBeCloseTo(width, 1);
  expect(bounds.height).toBeCloseTo(height, 1);
}

test.beforeEach(async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
});

test("reorders a list without navigating, and Undo restores its place", async ({ page }) => {
  const w = watch(page);
  await open(page);
  const work = group(page, "Work");
  await pickup(page, room(page, "Planning"));
  await dragOver(page, room(page, "Team"), 0.8);
  await expect(work.locator(".sidebar-after .nav-name")).toHaveText("Team");
  await page.mouse.up();
  await expect(names(work)).toHaveText(["Work", "Finance", "Team", "Planning"]);
  await expect(page.locator(".room-title")).toHaveText("My Day");
  await page.locator(".toast").getByRole("button", { name: /^Undo/ }).click();
  await expect(names(work)).toHaveText(["Work", "Planning", "Finance", "Team"]);
  await w.check();
});

test("drops between lists in another collection; a collapsed header opens on dwell", async ({ page }) => {
  const w = watch(page);
  await open(page);
  await nav(page).getByRole("button", { name: "Collapse Osobní" }).click();
  await pickup(page, room(page, "Planning"));
  await dragOver(page, room(page, "Osobní"));
  await expect(group(page, "Osobní").locator(".sidebar-into .nav-name")).toHaveText("Osobní");
  await expect(nav(page).getByRole("button", { name: "Collapse Osobní" })).toBeVisible();
  await dragOver(page, room(page, "Nákupy"), 0.2);
  await page.mouse.up();
  await expect(names(group(page, "Osobní"))).toHaveText(["Osobní", "Domácnost", "Planning", "Nákupy", "Zdraví"]);
  await pickup(page, room(page, "Planning"));
  await dragOver(page, room(page, "Reading list"), 0.8);
  await page.mouse.up();
  const loose = nav(page).locator(":scope > .nav-section").last();
  await expect(names(loose)).toHaveText(["Inbox", "Reading list", "Planning", "Garden"]);
  await w.check();
});

test("moves an entire collection and supports nudge, Escape, and fixed smart rooms", async ({ page }) => {
  const w = watch(page);
  await open(page);
  const heads = nav(page).locator(".nav-collection .nav-name");
  await pickup(page, room(page, "Work"));
  await dragOver(page, room(page, "Osobní"), 0.8);
  await expect(group(page, "Osobní")).toHaveClass(/sidebar-after/);
  await page.keyboard.press("Escape");
  await page.mouse.up();
  await expect(heads).toHaveText(["Work", "Osobní"]);
  await expect(page.locator(".sidebar-drag-lift")).toBeHidden();
  await pickup(page, room(page, "Work"));
  await dragOver(page, room(page, "Osobní"), 0.8);
  await page.mouse.up();
  await expect(heads).toHaveText(["Osobní", "Work"]);
  await expect(names(group(page, "Work"))).toHaveText(["Work", "Planning", "Finance", "Team"]);
  await room(page, "Work").press("Alt+ArrowUp");
  await expect(heads).toHaveText(["Work", "Osobní"]);
  await expect(room(page, "Work")).toBeFocused();
  await room(page, "Finance").press("Alt+ArrowUp");
  await expect(names(group(page, "Work"))).toHaveText(["Work", "Finance", "Planning", "Team"]);
  await pickup(page, room(page, "Work"));
  await dragOver(page, room(page, "My Day"));
  await expect(nav(page).locator(".sidebar-into,.sidebar-before,.sidebar-after")).toHaveCount(0);
  await page.mouse.up();
  await expect(heads).toHaveText(["Work", "Osobní"]);
  await w.check();
});

test("touch can move a list onto a collection header in the phone drawer", async ({ browser, baseURL }) => {
  const context = await browser.newContext({ baseURL, viewport: { width: 412, height: 920 }, hasTouch: true, isMobile: true });
  try {
    const page = await context.newPage();
    const w = watch(page);
    await open(page);
    await page.getByRole("button", { name: "Lists", exact: true }).click();
    const source = room(page, "Planning").locator(".sidebar-touch-grip");
    const destination = room(page, "Osobní");
    await source.scrollIntoViewIfNeeded();
    const src = await source.boundingBox();
    const dest = await destination.boundingBox();
    const cdp = await context.newCDPSession(page);
    const touch = (type, points = []) => cdp.send("Input.dispatchTouchEvent", { type, touchPoints: points });
    const x = src.x + src.width / 2;
    const y = src.y + src.height / 2;
    await touch("touchStart", [{ x, y }]);
    await touch("touchMove", [{ x, y: y + 8 }]);
    await expect(page.locator(".sidebar-drag-lift")).toBeVisible();
    await touch("touchMove", [{ x: dest.x + dest.width / 2, y: dest.y + dest.height / 2 }]);
    await expect(group(page, "Osobní").locator(".sidebar-into")).toBeVisible();
    await touch("touchEnd");
    await expect(names(group(page, "Osobní"))).toHaveText(["Osobní", "Domácnost", "Nákupy", "Zdraví", "Planning"]);
    await page.getByRole("dialog", { name: "Lists" }).getByRole("button", { name: /^Undo/ }).click();
    await expect(names(group(page, "Work"))).toHaveText(["Work", "Planning", "Finance", "Team"]);
    await w.check();
  } finally {
    await context.close();
  }
});

test("a held pointer near the edge scrolls only the room navigation", async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 485 });
  await open(page);
  await pickup(page, room(page, "Planning"));
  const bounds = await nav(page).boundingBox();
  const start = await nav(page).evaluate((node) => node.scrollTop);
  await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height - 4);
  await expect.poll(() => nav(page).evaluate((node) => node.scrollTop)).toBeGreaterThan(start);
  await page.keyboard.press("Escape");
  await page.mouse.up();
  await expect(names(group(page, "Work"))).toHaveText(["Work", "Planning", "Finance", "Team"]);
});

for (const colorScheme of ["light", "dark"]) {
  test.describe(`sidebar drag gallery ${colorScheme}`, () => {
    test.use({ colorScheme });
    test("list slot, collection target and collection slot", async ({ page }) => {
      const w = watch(page);
      await open(page);
      await pickup(page, room(page, "Planning"));
      await dragOver(page, room(page, "Finance"), 0.8);
      await expect(nav(page).locator(".sidebar-after .nav-name")).toHaveText("Finance");
      await expect(page.locator("main .eyebrow").first()).toContainText("21.11.");
      await page.screenshot({ path: `gallery/sidebar-drag-list-${colorScheme}.png`, animations: "disabled" });
      await dragOver(page, room(page, "Osobní"));
      await expect(nav(page).locator(".sidebar-into .nav-name")).toHaveText("Osobní");
      await page.screenshot({ path: `gallery/sidebar-drag-target-${colorScheme}.png`, animations: "disabled" });
      await page.keyboard.press("Escape");
      await page.mouse.up();
      await pickup(page, room(page, "Work"));
      await dragOver(page, room(page, "Osobní"), 0.8);
      await expect(group(page, "Osobní")).toHaveClass(/sidebar-after/);
      await page.screenshot({ path: `gallery/sidebar-drag-collection-${colorScheme}.png`, animations: "disabled" });
      await page.keyboard.press("Escape");
      await page.mouse.up();
      await w.check();
    });
  });
}
