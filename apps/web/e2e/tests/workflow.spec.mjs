import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";

const BASE = "/app/list/01930000-0000-7000-8002-000000000028";
const ROOM = `${BASE}?fixture=workflow`;
const DETAIL = `${BASE}/task/01930000-0000-7000-8001-000000000065?fixture=workflow`;
const TITLE = "Strip wallpaper and prime hallway walls";
const BACKLOG = "Compare kitchen pendant lights";
const TODO = "Order matte oak flooring samples";
const TODO_2 = "Confirm electrician visit for kitchen outlets";
const RECENT = "Deliver old sofa to charity depot";
const DESKTOP = { width: 1440, height: 900 };
const PHONE = { width: 412, height: 920 };
const column = (page, name) => page.locator(`[data-board-column="${name}"]`);
const card = (where, name) => where.getByRole("button", { name, exact: true });
const tab = (page, name) => page.getByRole("tab", { name, exact: true });
const picker = (page) => page.getByRole("listbox", { name: "Choose status" });
const option = (page, name) => picker(page).getByRole("option", { name, exact: true });
const undo = (page) => page.locator(".toast-undo");
const settings = (page) => page.getByRole("dialog", { name: "List settings" });

async function openSettings(page, location) {
  if (location) await location.click({ button: "right" });
  else await page.getByRole("button", { name: "Room menu", exact: true }).click();
  await page.getByRole("menuitem", { name: "List settings…", exact: true }).click();
  await expect(settings(page)).toBeVisible();
}

async function drag(page, from, to, fraction = 0.5, release = true) {
  const a = await from.boundingBox();
  await page.mouse.move(a.x + a.width / 2, a.y + a.height / 2);
  await page.mouse.down();
  await page.mouse.move(a.x + a.width / 2, a.y + a.height / 2 + 8);
  await expect(page.locator(".board-lift")).toBeVisible();
  const b = await to.boundingBox();
  await page.mouse.move(b.x + b.width / 2, b.y + b.height * fraction, { steps: 6 });
  if (release) await page.mouse.up();
}

test.beforeEach(async ({ page }) => {
  await page.setViewportSize(DESKTOP);
});

test("list settings are reachable from both menus; default applies on next visit", async ({ page }) => {
  const w = watch(page);
  await open(page, ROOM);
  await expect(tab(page, "Board")).toHaveAttribute("aria-selected", "true");
  await expect(tab(page, "Timeline")).toBeEnabled();
  await page.locator("#room-title").focus();
  await page.keyboard.press("V");
  await expect(tab(page, "Timeline")).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("V");
  await expect(tab(page, "List")).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("2");
  await expect(tab(page, "Board")).toHaveAttribute("aria-selected", "true");
  await openSettings(page);
  const dialog = settings(page);
  await expect(dialog.getByRole("switch", { name: "Workflow & Board" })).toBeChecked();
  await expect(dialog.getByRole("switch", { name: "Timeline & Start dates" })).toBeChecked();
  await dialog.getByRole("button", { name: "List", exact: true }).click();
  await expect(tab(page, "Board")).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menuitem", { name: "List settings…" })).toBeFocused();
  await page.keyboard.press("Escape");
  const sidebar = page.locator(".sidebar");
  const home = sidebar.getByRole("link", { name: /^Home renovation/ });
  const groceries = sidebar.getByRole("link", { name: /^Groceries/ });
  await groceries.click();
  await expect(page.getByRole("tablist", { name: "Room view" })).toHaveCount(0);
  await home.click();
  await expect(tab(page, "List")).toHaveAttribute("aria-selected", "true");
  await openSettings(page, home);
  await dialog.getByRole("switch", { name: "Workflow & Board" }).click();
  await dialog.getByRole("switch", { name: "Timeline & Start dates" }).click();
  await expect(dialog.getByRole("group", { name: "Default view" })).toHaveCount(0);
  await expect(page.getByRole("tablist", { name: "Room view" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Status: In progress" })).toHaveCount(0);
  await dialog.getByRole("switch", { name: "Workflow & Board" }).click();
  await dialog.getByRole("switch", { name: "Timeline & Start dates" }).click();
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await expect(page.getByRole("button", { name: "Status: In progress" })).toHaveCount(3);
  await expect(page.locator(".col-due", { hasText: "19.–23.11." })).toBeVisible();
  await tab(page, "Board").click();
  await openSettings(page);
  await dialog.getByRole("switch", { name: "Workflow & Board" }).click();
  await expect(tab(page, "List")).toHaveAttribute("aria-selected", "true");
  await expect(tab(page, "Board")).toBeHidden();
  await expect(tab(page, "Timeline")).toBeEnabled();
  await w.check();
});

test("row status, status shortcut, start-date reconciliation and composer suffixes", async ({ page }) => {
  const w = watch(page);
  await open(page, ROOM);
  await tab(page, "List").click();
  const row = page.locator(".row", { has: card(page, TITLE) });
  await row.getByRole("button", { name: "Status: In progress" }).click();
  await expect(option(page, "In progress")).toBeFocused();
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("Enter");
  await expect(row.getByRole("button", { name: "Status: To do" })).toBeVisible();
  await card(row, TITLE).click();
  await page.keyboard.press("Shift+S");
  const detail = page.locator("#task-detail");
  await expect(detail.locator("#edit-task-status")).toHaveText("In progress");
  await page.keyboard.press("Shift+D");
  const calendar = page.getByRole("dialog", { name: "Choose start date" });
  await calendar.getByRole("button", { name: "Next week", exact: true }).click();
  // A later date moves Due with it in the same undoable patch.
  await detail.getByRole("button", { name: "Edit start date" }).click();
  await calendar.getByRole("button", { name: /Friday,? 27 November 2026/ }).click();
  await expect(detail.locator("#edit-task-start")).toHaveText("Fri 27.11.");
  await expect(detail.locator("#edit-task-due")).toHaveText("Fri 27.11.");
  await undo(page).click();
  await expect(detail.locator("#edit-task-due")).toHaveText("Mon 23.11.");

  const input = page.getByRole("textbox", { name: "Add a task" });
  await input.fill("Paint the door tomorrow..+4d !! stav:rozpracovano");
  await expect(page.getByRole("button", { name: "Remove status: In progress" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Remove dates: 22.–25.11." })).toBeVisible();
  await page.locator("#qa-status").click();
  await option(page, "Backlog").click();
  await expect(input).toHaveValue("Paint the door tomorrow..+4d !!");
  await input.press("Enter");
  const added = page.locator(".row", { has: card(page, "Paint the door") });
  await expect(added.getByRole("button", { name: "Status: Backlog" })).toBeVisible();
  await expect(added.locator(".col-due")).toHaveText("22.–25.11.");
  await page.locator(".sidebar").getByRole("link", { name: /^All / }).click();
  await expect(page.getByRole("button", { name: "Status: To do" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Status: Backlog" })).toHaveCount(3);
  await w.check();
});

test("Board pointer moves, in-column reorder, cancellation and Undo", async ({ page }) => {
  const w = watch(page);
  await open(page, ROOM);
  const backlog = column(page, "backlog");
  const todo = column(page, "todo");
  const progress = column(page, "in-progress");
  await expect(column(page, "done").locator(".board-title")).toHaveCount(3);
  expect(await page.locator(".board").evaluate((el) => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(1);
  await drag(page, card(backlog, BACKLOG), progress.getByRole("heading"), 0.5, false);
  await expect(progress.locator(".board-last-slot")).toHaveClass(/show-slot/);
  await page.keyboard.press("Escape");
  await page.mouse.up();
  await expect(card(backlog, BACKLOG)).toBeVisible();
  await expect(page.locator("#task-detail")).not.toBeVisible();
  await drag(page, card(backlog, BACKLOG), card(progress, TITLE), 0.2);
  await expect(card(progress, BACKLOG)).toBeVisible();
  await expect(progress.locator(".board-title").first()).toHaveText(BACKLOG);
  await undo(page).click();
  await expect(card(backlog, BACKLOG)).toBeVisible();
  await drag(page, card(todo, TODO), card(todo, TODO_2), 0.9);
  await expect(todo.locator(".board-title").first()).toHaveText(TODO_2);
  await undo(page).click();
  await expect(todo.locator(".board-title").first()).toHaveText(TODO);
  await w.check();
});

test("Board keys navigate without opening detail, move, reorder, add and set bulk Status", async ({ page }) => {
  const w = watch(page);
  await open(page, ROOM);
  const backlog = column(page, "backlog");
  const todo = column(page, "todo");
  await page.locator("#room-title").focus();
  await page.keyboard.press("H");
  await expect(card(backlog, BACKLOG)).toBeFocused();
  await page.keyboard.press("L");
  await expect(card(todo, TODO)).toBeFocused();
  await expect(page.locator("#task-detail")).not.toBeVisible();
  await page.keyboard.press("Alt+ArrowDown");
  await expect(todo.locator(".board-title").first()).toHaveText(TODO_2);
  await expect(card(todo, TODO)).toBeFocused();
  await page.keyboard.press("Control+Alt+ArrowLeft");
  await expect(card(backlog, TODO)).toBeFocused();
  await page.keyboard.press("Control+z");
  await expect(card(todo, TODO)).toBeFocused();

  await card(backlog, BACKLOG).focus();
  await page.keyboard.press("N");
  const composer = backlog.getByRole("textbox", { name: "Add to Backlog" });
  await expect(composer).toBeFocused();
  await composer.fill("Collect paint swatches tomorrow");
  await composer.press("Enter");
  await expect(backlog.locator(".board-title").first()).toHaveText("Collect paint swatches");
  await composer.press("Escape");
  await expect(backlog.getByRole("button", { name: "Add to Backlog" })).toBeFocused();
  await card(backlog, BACKLOG).click({ modifiers: ["Control"] });
  await card(todo, TODO).click({ modifiers: ["Control"] });
  await page.getByRole("button", { name: "Status", exact: true }).click();
  await option(page, "In progress").click();
  await expect(card(column(page, "in-progress"), BACKLOG)).toBeVisible();
  await expect(card(column(page, "in-progress"), TODO)).toBeVisible();
  await undo(page).click();
  await expect(card(backlog, BACKLOG)).toBeVisible();
  await expect(card(todo, TODO)).toBeVisible();
  await w.check();
});

test("Done rail remains a drop target beside detail; completion and reopening undo", async ({ page }) => {
  const w = watch(page);
  await open(page, DETAIL);
  const progress = column(page, "in-progress");
  const done = column(page, "done");
  const rail = done.getByRole("button", { name: /Expand Done/ });
  await expect(rail).toBeInViewport();
  await expect(done).toHaveCSS("width", "44px");
  await drag(page, card(progress, TITLE), rail, 0.5);
  await expect(page.locator(".toast")).toContainText("Completed");
  await expect(rail).toHaveAccessibleName("Expand Done, 15 completed tasks");
  await undo(page).click();
  await expect(card(progress, TITLE)).toBeVisible();
  await expect(rail).toHaveAccessibleName("Expand Done, 14 completed tasks");
  await rail.click();
  await expect(done.getByRole("button", { name: "Collapse Done" })).toBeVisible();
  await card(done, RECENT).focus();
  await page.keyboard.press("Control+Alt+ArrowLeft");
  await expect(card(progress, RECENT)).toBeVisible();
  await undo(page).click();
  await expect(card(done, RECENT)).toBeVisible();
  await done.getByRole("button", { name: "View all 14 in Completed →" }).click();
  await expect(tab(page, "List")).toHaveAttribute("aria-selected", "true");
  await expect(page.locator(".completed-rows .row")).toHaveCount(14);
  await w.check();
});

test("narrow Board scrolls inside the room, uses icon tabs, and opens modal detail", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize(PHONE);
  await open(page, ROOM);
  const board = page.locator(".board");
  await expect(tab(page, "Board").locator(".icon")).toBeVisible();
  await expect(tab(page, "Board").getByText("Board", { exact: true })).toBeHidden();
  expect(await board.evaluate((el) => el.scrollWidth > el.clientWidth)).toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(PHONE.width);
  await card(column(page, "in-progress"), TITLE).click();
  const detail = page.getByRole("dialog", { name: "Task detail" });
  await expect(detail).toBeVisible();
  await detail.locator("#edit-task-status").click();
  await option(page, "Backlog").click();
  await expect(detail.locator("#edit-task-status")).toHaveText("Backlog");
  await page.keyboard.press("Escape");
  await expect(detail).not.toBeVisible();
  await expect(card(column(page, "backlog"), TITLE)).toBeFocused();
  await w.check();
});

for (const scheme of ["light", "dark"]) {
  test.describe(`Board gallery in ${scheme}`, () => {
    test.use({ colorScheme: scheme });
    test("Board, Done rail, settings, List status and phone Board", async ({ page }) => {
      const w = watch(page);
      const shot = (name) => page.screenshot({ path: `gallery/${name}-${scheme}.png`, animations: "disabled" });
      await open(page, ROOM);
      await shot("board");
      await drag(page, card(column(page, "todo"), TODO), column(page, "in-progress").getByRole("heading"), 0.5, false);
      await expect(page.locator(".board-lift")).toBeVisible();
      await shot("board-drag");
      await page.mouse.up();
      await expect(undo(page)).toBeVisible();
      await expect(page.locator(".toast")).toHaveCSS("opacity", "1");
      await shot("board-move");
      await undo(page).click();
      await page.locator(".toast-close").click();
      const selectedCard = card(column(page, "in-progress"), TITLE);
      await selectedCard.click();
      await expect(page.locator("#edit-task-start")).toBeVisible();
      const rail = await selectedCard.evaluate((el) => {
        const style = getComputedStyle(el.closest(".board-card"), "::before");
        return [style.insetInlineStart, style.insetBlockStart, style.insetBlockEnd, style.borderTopLeftRadius];
      });
      expect(rail).toEqual(["4px", "4px", "4px", "4px"]);
      await shot("board-detail");
      await tab(page, "List").click();
      await page.getByRole("textbox", { name: "Add a task" }).fill("Install hallway sconces");
      await page.locator("#qa-date").click();
      await page.getByRole("button", { name: "Saturday, 28 November 2026" }).click();
      await page.locator("#qa-date").click();
      await page.getByRole("dialog", { name: "Choose due date" }).getByRole("button", { name: "Starts", exact: true }).click();
      await page.getByRole("dialog", { name: "Choose start date" }).getByRole("button", { name: "Thursday, 26 November 2026" }).click();
      await page.locator("#qa-status").click();
      await option(page, "In progress").click();
      await expect(page.getByRole("button", { name: "Remove status: In progress" })).toBeVisible();
      await expect(page.getByRole("button", { name: "Remove dates: 26.–28.11." })).toBeVisible();
      await shot("list-status");
      await open(page, ROOM);
      await tab(page, "List").click();
      await openSettings(page);
      await shot("list-settings");
      await page.keyboard.press("Escape");
      await page.keyboard.press("Escape");
      await tab(page, "Board").click();
      await page.setViewportSize(PHONE);
      await expect(card(column(page, "backlog"), BACKLOG)).toBeVisible();
      await shot("board-phone");
      await w.check();
    });
  });
}
