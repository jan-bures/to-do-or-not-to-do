import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";

const board = "Prepare the November board update";
const hiring = "Outline the 2027 hiring plan";
const row = (page, title) => page.locator(".row-title").filter({ hasText: title });
const menu = (page) => page.getByRole("menu", { name: "Task actions" });
const picker = (page) => page.getByRole("dialog", { name: "Move to…" });
const toast = (page) => page.locator(".toast");

async function planning(page) {
  await page.setViewportSize({ width: 1440, height: 900 });
  await open(page);
  await page.locator(".sidebar").getByRole("link", { name: /^Planning/ }).click();
}

async function watchExits(page) {
  await page.evaluate(() => {
    window.taskExits = [];
    new MutationObserver((records) => {
      for (const { addedNodes } of records) {
        for (const node of addedNodes) {
          if (node instanceof Element && node.matches("[data-task-row].exiting[inert]")) {
            window.taskExits.push(getComputedStyle(node).animationName);
          }
        }
      }
    }).observe(document.querySelector(".room"), { childList: true, subtree: true });
  });
}

test("row context delete exits, hands off focus and offers exact Undo", async ({ page }) => {
  const w = watch(page);
  await planning(page);
  await row(page, board).click();
  await watchExits(page);
  await row(page, board).click({ button: "right" });
  await menu(page).getByRole("menuitem", { name: "Delete task" }).click();
  await expect(menu(page)).toBeHidden();
  await expect(toast(page)).toContainText(`Deleted “${board}”`);
  await expect(row(page, hiring)).toBeFocused();
  await expect(row(page, board)).toHaveCount(0);
  expect(await page.evaluate(() => window.taskExits)).toEqual(["row-exit"]);
  await toast(page).getByRole("button", { name: "Undo" }).click();
  await expect(row(page, board)).toBeVisible();
  await w.check();
});

test("Delete ignores editors and repeats; Ctrl+Z restores the selected row with reduced motion", async ({ page }) => {
  const w = watch(page);
  await page.emulateMedia({ reducedMotion: "reduce" });
  await planning(page);
  await row(page, board).click();
  await page.keyboard.press("e");
  const editor = page.locator(".title-input");
  await expect(editor).toBeFocused();
  await editor.press("Delete");
  await expect(toast(page)).toHaveCount(0);
  await editor.press("Escape");
  await expect(page.locator("#edit-task-title")).toBeFocused();

  await row(page, board).focus();
  await expect(row(page, board)).toBeFocused();
  await watchExits(page);
  await page.keyboard.down("Delete");
  await expect(row(page, hiring)).toBeFocused();
  await page.keyboard.down("Delete");
  await page.keyboard.up("Delete");
  await expect(row(page, hiring)).toBeVisible();
  await expect(row(page, board)).toHaveCount(0);
  expect(await page.evaluate(() => window.taskExits)).toEqual([]);
  await page.keyboard.press("Control+z");
  await expect(row(page, board)).toBeVisible();
  await w.check();
});

test("keyboard move from a list filters grouped destinations, skips the current list and undoes", async ({ page }) => {
  const w = watch(page);
  await planning(page);
  await row(page, board).click();
  await page.keyboard.press("Shift+F10");
  const move = menu(page).getByRole("menuitem", { name: "Move to…" });
  await expect(move).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(menu(page).getByRole("menuitem", { name: "Delete task" })).toBeFocused();
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("Enter");
  const filter = picker(page).getByRole("textbox", { name: "Filter lists" });
  await expect(filter).toBeFocused();
  await expect(picker(page).getByRole("group", { name: "Work" }).locator("[data-list-hue=azure] svg")).toBeVisible();
  await expect(picker(page).getByRole("option", { name: "Planning", selected: true })).toBeDisabled();
  await expect(picker(page).getByRole("option", { name: "Planning" }).locator("[data-list-hue=cornflower] svg")).toBeVisible();
  await filter.press("ArrowDown");
  await expect(picker(page).getByRole("option", { name: "Finance" })).toBeFocused();
  await picker(page).getByRole("option", { name: "Finance" }).press("Shift+Tab");
  await expect(filter).toBeFocused();
  await filter.fill("no such list");
  await expect(picker(page).getByRole("status")).toHaveText("No list matches “no such list”.");
  await filter.fill("osobni DOMÁC");
  await expect(picker(page).getByRole("option")).toHaveCount(1);
  await filter.press("ArrowDown");
  await expect(picker(page).getByRole("option", { name: "Domácnost" })).toBeFocused();
  await watchExits(page);
  await page.keyboard.press("Enter");
  await expect(picker(page)).toBeHidden();
  await expect(row(page, hiring)).toBeFocused();
  await expect(row(page, board)).toHaveCount(0);
  await expect(toast(page)).toContainText(`Moved “${board}” to Domácnost`);
  expect(await page.evaluate(() => window.taskExits)).toEqual(["row-exit"]);
  await toast(page).getByRole("button", { name: "Undo" }).click();
  await expect(row(page, board)).toBeVisible();
  await w.check();
});

test("M and Escape restore focus; moving in My Day keeps the task selected", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await open(page);
  await row(page, board).click();
  await page.keyboard.press("m");
  await expect(picker(page).getByRole("textbox", { name: "Filter lists" })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(row(page, board)).toBeFocused();

  await page.keyboard.press("m");
  await picker(page).getByRole("option", { name: "Finance" }).click();
  await expect(row(page, board)).toBeFocused();
  await expect(page.locator(".crumb-list")).toHaveText("Finance");
  await page.keyboard.press("Control+z");
  await expect(page.locator(".crumb-list")).toHaveText("Planning");
  await w.check();
});

test("completed and hand-over rows can be deleted and restored", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await open(page, "/app?fixture=finished");
  await page.getByRole("button", { name: "Completed 12" }).click();
  const done = page.getByRole("list", { name: "Completed tasks" });
  const firstDone = await done.locator(".row-title").first().textContent();
  const nextDone = await done.locator(".row-title").nth(1).textContent();
  await row(page, firstDone).click();
  await page.keyboard.press("Delete");
  await expect(row(page, firstDone)).toHaveCount(0);
  await expect(row(page, nextDone)).toBeFocused();
  await page.keyboard.press("Control+z");
  await expect(row(page, firstDone)).toBeVisible();
  await page.getByRole("button", { name: "Completed 12" }).click();

  const handover = page.locator(".tomorrow");
  const first = handover.getByRole("button", { name: "Připravit nedělní oběd" });
  const next = handover.getByRole("button", { name: board });
  await first.click();
  await page.keyboard.press("Delete");
  await expect(first).toHaveCount(0);
  await expect(next).toBeFocused();
  await page.keyboard.press("Control+z");
  await expect(first).toBeVisible();
  await w.check();
});

test("deleting the last open task keeps focus on the finished room heading", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await open(page, "/app?fixture=finished");
  const add = page.getByRole("textbox", { name: "Add a task" });
  await add.fill("One last task");
  await add.press("Enter");
  await row(page, "One last task").click();
  await page.keyboard.press("Delete");
  await expect(page.locator(".room-head.finished .room-title")).toBeFocused();
  await w.check();
});

test("phone detail overflow restores focus on Cancel and can delete with Undo", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 412, height: 920 });
  await open(page);
  await row(page, board).click();
  const button = page.getByRole("button", { name: "Task actions" });
  await button.click();
  await menu(page).getByRole("menuitem", { name: "Move to…" }).click();
  await picker(page).getByRole("button", { name: "Cancel" }).click();
  await expect(button).toBeFocused();
  await button.click();
  await menu(page).getByRole("menuitem", { name: "Delete task" }).click();
  await expect(page.getByRole("dialog", { name: "Task detail" })).toBeHidden();
  await expect(page.locator(".row-title:focus")).toHaveCount(1);
  await expect(row(page, board)).toHaveCount(0);
  await toast(page).getByRole("button", { name: "Undo" }).click();
  await expect(row(page, board)).toBeVisible();
  await w.check();
});
