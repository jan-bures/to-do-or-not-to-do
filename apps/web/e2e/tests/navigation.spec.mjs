import { expect, test } from "@playwright/test";
import { open, openRows, watch } from "./helpers.mjs";

const room = (page) => page.locator(".room-title");
const detail = (page) => page.locator(".detail .d-title");
const nav = (page, name) => page.locator(".sidebar").getByRole("link", { name });
const pickView = (page, name) => page.getByRole("tab", { name, exact: true });

test.beforeEach(async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
});

test("Back and Forward restore rooms and explicit detail opens and closes", async ({ page }) => {
  const w = watch(page);
  await open(page);
  await nav(page, /^Important/).click();
  await nav(page, /^Finance/).click();
  const finance = page.url();
  const task = openRows(page).first().locator(".row-title");
  const title = await task.innerText();
  await task.click();
  await expect(detail(page)).toHaveText(title);
  const taskUrl = page.url();
  await page.locator("#task-detail").getByRole("button", { name: "Close", exact: true }).click();
  await expect(page).toHaveURL(finance);

  await page.goBack();
  await expect(page).toHaveURL(taskUrl);
  await expect(detail(page)).toHaveText(title);
  await page.goBack();
  await expect(page).toHaveURL(finance);
  await expect(detail(page)).toBeHidden();
  await page.goBack();
  await expect(room(page)).toHaveText("Important");
  await page.goForward();
  await expect(room(page)).toHaveText("Finance");
  await page.goForward();
  await expect(detail(page)).toHaveText(title);
  await page.goForward();
  await expect(detail(page)).toBeHidden();
  await w.check();
});

test("history skips a list deleted since the visit, in both directions", async ({ page }) => {
  const w = watch(page);
  await open(page);
  const finance = nav(page, /^Finance/);
  await finance.click();
  await expect(room(page)).toHaveText("Finance");
  await nav(page, /^Important/).click();
  await finance.click({ button: "right" });
  await page.getByRole("menuitem", { name: "Delete list", exact: true }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Delete", exact: true }).click();
  await expect(finance).toBeHidden();

  await page.goBack();
  await expect(room(page)).toHaveText("My Day");
  await page.goForward();
  await expect(room(page)).toHaveText("Important");
  await w.check();
});

test("filtering, keyboard previews and duplicate room visits do not add entries", async ({ page }) => {
  const w = watch(page);
  await open(page);
  const all = nav(page, /^All\b/);
  await all.click();
  await expect(room(page)).toHaveText("All");
  const length = await page.evaluate(() => history.length);
  await all.click();
  await page.getByRole("button", { name: "Filter tasks" }).click();
  await page.locator("#room-filter").pressSequentially("backup");
  await expect(page.locator("#room-filter")).toHaveValue("backup");
  await page.getByRole("button", { name: "Close filter" }).click();
  await page.locator("#room-title").focus();
  await page.keyboard.press("j");
  await expect(detail(page)).toBeVisible();
  expect(await page.evaluate(() => history.length)).toBe(length);
  await expect(page).toHaveURL(/\/app\/all$/);
  await page.goBack();
  await expect(room(page)).toHaveText("My Day");
  await page.goForward();
  await expect(room(page)).toHaveText("All");
  await expect(detail(page)).toBeHidden();
  await w.check();
});

test("a workflow view with task detail restores through history and on reload", async ({ page }) => {
  const w = watch(page);
  await open(page, "/app/list/01930000-0000-7000-8002-000000000028?fixture=workflow");
  await expect(pickView(page, "Board")).toHaveAttribute("aria-selected", "true");
  await pickView(page, "Timeline").click();
  await pickView(page, "List").click();
  const title = "Strip wallpaper and prime hallway walls";
  await openRows(page).getByRole("button", { name: title, exact: true }).click();
  await expect(detail(page)).toHaveText(title);
  await expect(page).toHaveURL(/\/view\/list\/task\/[^?]+\?fixture=workflow$/);

  await page.goBack();
  await expect(pickView(page, "List")).toHaveAttribute("aria-selected", "true");
  await expect(detail(page)).toBeHidden();
  await page.goBack();
  await expect(pickView(page, "Timeline")).toHaveAttribute("aria-selected", "true");
  await page.goForward();
  await expect(pickView(page, "List")).toHaveAttribute("aria-selected", "true");
  await page.goForward();
  await expect(detail(page)).toHaveText(title);
  await page.reload();
  await expect(pickView(page, "List")).toHaveAttribute("aria-selected", "true");
  await expect(detail(page)).toHaveText(title);
  await w.check();
});
