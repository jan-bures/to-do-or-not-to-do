import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";

const rows = (page) => page.locator(".rows:not(.completed-rows) .row-title");
const selected = (page) => page.locator('.row-title[aria-pressed="true"]');
const bulk = (page) => page.getByRole("region", { name: "Task selection" });

async function pickTwo(page) {
  await rows(page).nth(0).click({ modifiers: ["Control"] });
  await rows(page).nth(1).click({ modifiers: ["Control"] });
  await expect(bulk(page)).toContainText("2 tasks selected");
}

test("modifier and keyboard selection follows the visible room order", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await open(page);
  const tasks = rows(page);
  const names = await tasks.allTextContents();
  await tasks.nth(0).click();
  await tasks.nth(3).click({ modifiers: ["Shift"] });
  await expect(selected(page)).toHaveText(names.slice(0, 4));
  await expect(tasks.nth(3)).toBeFocused();
  await page.keyboard.press("Shift+ArrowUp");
  await expect(selected(page)).toHaveText(names.slice(0, 3));
  await tasks.nth(1).click({ modifiers: ["Meta"] });
  await expect(selected(page)).toHaveText([names[0], names[2]]);
  await page.keyboard.press("Control+a");
  await expect(selected(page)).toHaveText(names);
  await page.keyboard.press("Escape");
  await expect(bulk(page)).toBeHidden();
  await expect(selected(page)).toHaveCount(0);

  // Ctrl+A in a text field selects text, not tasks. Hiding rows clears their membership.
  await pickTwo(page);
  await page.keyboard.press("Control+k");
  const filter = page.locator("#room-filter");
  await filter.fill("nothing matches this");
  await expect(bulk(page)).toBeHidden();
  await filter.press("Control+a");
  await expect(bulk(page)).toBeHidden();
  await filter.fill("");
  await expect(tasks).toHaveCount(names.length);
  await expect(selected(page)).toHaveCount(0);
  await w.check();
});

test("bulk actions expose one Undo each", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await open(page);
  const names = (await rows(page).allTextContents()).slice(0, 2);
  const task = (n) => page.getByRole("button", { name: n, exact: true });
  const undo = () => page.locator(".toast").getByRole("button", { name: /^Undo\b/ }).click({ timeout: 5000 });
  const toast = (text) => expect(page.locator(".toast-text")).toHaveText(text);

  await pickTwo(page);
  await bulk(page).getByRole("button", { name: "Complete", exact: true }).click();
  await toast("Completed 2 tasks");
  for (const n of names) await expect(task(n)).toBeHidden();
  await undo();
  for (const n of names) await expect(task(n)).toBeVisible();

  await pickTwo(page);
  await bulk(page).getByRole("button", { name: "Set due…" }).click();
  const due = page.getByRole("dialog", { name: "Due date for selected tasks" });
  await due.getByRole("textbox", { name: "Due date" }).fill("+5d");
  await due.getByRole("button", { name: "Save", exact: true }).click();
  await toast("Set due date for 2 tasks");
  await undo();

  await pickTwo(page);
  await bulk(page).getByRole("button", { name: "Move to…" }).click();
  const move = page.getByRole("dialog", { name: "Move to…" });
  await move.getByRole("textbox", { name: "Filter lists" }).fill("Inbox");
  await move.getByRole("option", { name: "Inbox" }).click();
  await toast("Moved 2 tasks");
  await undo();

  await pickTwo(page);
  await bulk(page).getByRole("button", { name: "Mark important" }).click();
  await toast("Marked 1 task important");
  await undo();

  await pickTwo(page);
  await bulk(page).getByRole("button", { name: "Remove from My Day" }).click();
  await toast("Removed 2 tasks from My Day");
  for (const n of names) await expect(task(n)).toBeHidden();
  await undo();
  for (const n of names) await expect(task(n)).toBeVisible();

  await pickTwo(page);
  await bulk(page).getByRole("button", { name: "Delete" }).click();
  await toast("Deleted 2 tasks");
  for (const n of names) await expect(task(n)).toBeHidden();
  await undo();
  for (const n of names) await expect(task(n)).toBeVisible();
  await w.check();
});
