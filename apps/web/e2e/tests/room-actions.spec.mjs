import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";

test.use({ viewport: { width: 1440, height: 900 } });

const nav = (page) => page.getByRole("navigation").filter({ has: page.getByRole("link", { name: /^Agent activity/ }) });
const item = (page, name) => nav(page).getByRole("link", { name: new RegExp(`^${name}( |$)`) });
const menu = (page) => page.getByRole("menu", { name: "Room actions" });
const action = (page, name) => menu(page).getByRole("menuitem", { name, exact: true });
const roomMenu = (page) => page.getByRole("button", { name: "Room menu" });

test("rename from the sidebar and room menus, with keyboard focus and cancel", async ({ page }) => {
  const w = watch(page);
  await open(page);
  await item(page, "Finance").press("Shift+F10");
  await expect(action(page, "Rename list")).toBeFocused();
  await page.keyboard.down("ArrowDown");
  await page.keyboard.down("ArrowDown");
  await page.keyboard.up("ArrowDown");
  await expect(action(page, "Move up")).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(item(page, "Finance")).toBeFocused();
  await item(page, "Finance").click({ button: "right" });
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("My Day");
  await action(page, "Rename list").press("Enter");
  const listInput = page.getByRole("textbox", { name: "list name" });
  await expect(listInput).toBeFocused();
  await expect(listInput).toHaveValue("Finance");
  await listInput.fill("F".repeat(201));
  await listInput.press("Enter");
  await expect(page.getByRole("dialog").getByRole("alert")).toContainText("name longer than 200 characters");
  await listInput.fill("Budgets");
  await listInput.press("Control+k");
  await expect(listInput).toBeFocused();
  await listInput.press("Enter");
  await expect(item(page, "Budgets")).toBeFocused();
  await expect(item(page, "Finance")).toHaveCount(0);

  await item(page, "Work").click();
  await roomMenu(page).click();
  await action(page, "Rename collection").click();
  const collection = page.getByRole("textbox", { name: "collection name" });
  await collection.fill("Discard this");
  await collection.press("Escape");
  await expect(roomMenu(page)).toBeFocused();
  await roomMenu(page).click();
  await action(page, "Rename collection").click();
  await collection.fill("Studio");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Studio");
  await expect(item(page, "Studio")).toBeVisible();
  await w.check();
});

test("list and collection moves stay in their scope and are undoable", async ({ page }) => {
  const w = watch(page);
  await open(page);
  const lists = page.locator(".nav-section").filter({ has: page.getByRole("link", { name: /^Work( |$)/ }) }).locator(".nav-item.sub .nav-name");
  const collections = page.locator(".nav-collection .nav-name");
  await expect(lists).toHaveText(["Planning", "Finance", "Team"]);
  await item(page, "Planning").click({ button: "right" });
  await expect(action(page, "Move up")).toBeDisabled();
  await action(page, "Move down").click();
  await expect(lists).toHaveText(["Finance", "Planning", "Team"]);
  await page.getByRole("button", { name: /^Undo/ }).click();
  await expect(lists).toHaveText(["Planning", "Finance", "Team"]);
  await item(page, "Osobní").click();
  await roomMenu(page).press("Enter");
  await expect(action(page, "Move down")).toBeDisabled();
  await action(page, "Move up").click();
  await expect(collections).toHaveText(["Osobní", "Work"]);
  await page.keyboard.press("Control+z");
  await expect(collections).toHaveText(["Work", "Osobní"]);
  await w.check();
});

test("delete uses a scrim and safe focus, and Undo brings back children", async ({ page }) => {
  const w = watch(page);
  await open(page);
  await item(page, "Planning").click();
  await roomMenu(page).click();
  await action(page, "Delete list").click();
  const listDialog = page.getByRole("dialog", { name: "Delete list" });
  await expect(listDialog).toContainText("Delete “Planning”?");
  await expect(listDialog.getByRole("button", { name: "Cancel" })).toBeFocused();
  await expect.poll(() => listDialog.evaluate((el) => getComputedStyle(el, "::backdrop").opacity)).toBe("1");
  await page.mouse.click(20, 20);
  await expect(listDialog).toBeHidden();
  await expect(roomMenu(page)).toBeFocused();
  await roomMenu(page).click();
  await action(page, "Delete list").click();
  await listDialog.getByRole("button", { name: "Delete", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Work");
  await expect(item(page, "Planning")).toHaveCount(0);
  await page.getByRole("button", { name: /^Undo/ }).click();
  await expect(item(page, "Planning")).toBeVisible();

  await item(page, "Planning").click();
  await item(page, "Work").click({ button: "right" });
  await action(page, "Delete collection").click();
  const collectionDialog = page.getByRole("dialog", { name: "Delete collection" });
  await expect(collectionDialog).toContainText("Its lists and tasks will be hidden too");
  await collectionDialog.getByRole("button", { name: "Delete", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("My Day");
  await expect(item(page, "Work")).toHaveCount(0);
  await expect(item(page, "Planning")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Prepare the November board update", exact: true })).toHaveCount(0);
  await page.keyboard.press("Control+z");
  await expect(item(page, "Work")).toBeVisible();
  await expect(item(page, "Planning")).toBeVisible();
  await expect(page.getByRole("button", { name: "Prepare the November board update", exact: true })).toBeVisible();
  await w.check();
});

test("the phone can open sidebar actions without right clicking", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 412, height: 920 });
  await open(page);
  await page.getByRole("button", { name: "Lists", exact: true }).click();
  await page.getByRole("button", { name: "Menu for Work" }).click();
  await expect(action(page, "Rename collection")).toBeFocused();
  await action(page, "Collapse collection").click();
  await expect(item(page, "Planning")).toBeHidden();
  await w.check();
});

test("held Tab stays in the phone drawer, and a move returns focus to its original menu button", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 412, height: 920 });
  await open(page);
  await page.getByRole("button", { name: "Lists", exact: true }).click();
  const drawer = page.getByRole("dialog", { name: "Lists" });
  await expect(drawer).toBeFocused();
  const first = drawer.getByRole("link", { name: /^My Day/ });
  const last = drawer.getByRole("button", { name: "Settings" });
  await last.focus();
  await page.keyboard.down("Tab");
  await expect(first).toBeFocused();
  await last.focus();
  await page.keyboard.down("Tab");
  await page.keyboard.up("Tab");
  await expect(first).toBeFocused();
  await last.press("Enter");
  await expect(drawer.getByRole("menuitem", { name: "Devices…" })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(first).toBeFocused();

  const move = drawer.getByRole("button", { name: "Menu for Work" });
  const collections = drawer.locator(".nav-collection .nav-name");
  await move.click();
  await action(page, "Move down").click();
  await expect(collections).toHaveText(["Osobní", "Work"]);
  await expect(move).toBeFocused();
  await page.keyboard.press("Control+z");
  await expect(collections).toHaveText(["Work", "Osobní"]);
  await w.check();
});

test("the menu stays by its trigger and flips its origin when space runs out", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 412, height: 660 });
  await open(page);
  await page.getByRole("button", { name: "Lists", exact: true }).click();
  const drawer = page.getByRole("dialog", { name: "Lists" });
  await expect(drawer).toHaveCSS("transform", "none");
  const trigger = drawer.getByRole("button", { name: "Menu for Garden" });
  await trigger.evaluate((el) => {
    const nav = el.closest("nav");
    nav.scrollTop += el.getBoundingClientRect().bottom - nav.getBoundingClientRect().bottom + 8;
  });
  await trigger.press("Enter");
  await expect(menu(page)).toHaveClass(/above-anchor/);
  await expect(menu(page)).toHaveClass(/end-anchor/);
  await expect(menu(page)).toHaveCSS("transform", "none");
  const bounds = await trigger.boundingBox();
  const pop = await menu(page).boundingBox();
  expect(Math.abs(pop.y + pop.height - bounds.y)).toBeLessThan(2);
  expect(Math.abs(pop.x + pop.width - bounds.x - bounds.width)).toBeLessThan(2);
  await w.check();
});

test("Not synced lists safe reasons, dismisses one or all, and restores focus", async ({ page }) => {
  const w = watch(page);
  await open(page, "/app?fixture=not-synced");
  const trigger = page.getByRole("button", { name: "2 couldn't sync" });
  await trigger.press("Enter");
  const dialog = page.getByRole("dialog", { name: "Not synced" });
  await expect(dialog.getByRole("listitem")).toHaveCount(2);
  await expect(dialog).toContainText("Couldn't undo: the item was changed again.");
  await dialog.getByRole("listitem").filter({ hasText: "Museum visits" }).getByRole("button", { name: "Dismiss", exact: true }).click();
  await expect(dialog.getByRole("listitem")).toHaveCount(1);
  await expect(dialog.getByRole("listitem")).toContainText("Undo a change");
  await dialog.getByRole("button", { name: "Dismiss all" }).press("Enter");
  await expect(dialog).toContainText("No more changes to dismiss.");
  await expect(dialog.getByRole("button", { name: "Close", exact: true })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(page.getByRole("button", { name: "New", exact: true })).toBeFocused();
  await expect(page.locator(".sync-text")).not.toContainText("not synced");
  await w.check();
});

test("folded collections alone are remembered across reloads", async ({ page }) => {
  const w = watch(page);
  await open(page);
  await page.getByRole("button", { name: "Collapse Work" }).click();
  await expect(item(page, "Planning")).toBeHidden();
  await page.reload();
  await expect(page.getByRole("button", { name: "Expand Work" })).toHaveAttribute("aria-expanded", "false");
  await expect(item(page, "Planning")).toBeHidden();
  await expect(item(page, "Domácnost")).toBeVisible();
  await page.getByRole("button", { name: "Expand Work" }).click();
  await page.reload();
  await expect(item(page, "Planning")).toBeVisible();
  await w.check();
});

test("tabs preserve each other's folds and remove stale collection IDs", async ({ page, context }) => {
  const missing = "00000000-0000-7000-8000-000000000999";
  const key = `todo-folded-collection:${missing}`;
  await page.addInitScript((key) => localStorage.setItem(key, "1"), key);
  await open(page);
  const other = await context.newPage();
  await open(other);
  await expect.poll(() => page.evaluate((key) => localStorage.getItem(key), key)).toBeNull();
  await Promise.all([
    page.getByRole("button", { name: "Collapse Work" }).click(),
    other.getByRole("button", { name: "Collapse Osobní" }).click(),
  ]);
  for (const tab of [page, other]) {
    await expect(tab.getByRole("button", { name: "Expand Work" })).toBeVisible();
    await expect(tab.getByRole("button", { name: "Expand Osobní" })).toBeVisible();
  }
  await other.reload();
  await other.getByRole("button", { name: "Expand Osobní" }).click();
  await expect(page.getByRole("button", { name: "Collapse Osobní" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Expand Work" })).toBeVisible();
});

test("folding still works when the browser denies localStorage reads and writes", async ({ page }) => {
  const w = watch(page);
  await page.addInitScript(() => {
    Object.defineProperty(window, "localStorage", {
      get() { throw new DOMException("Denied", "SecurityError"); },
    });
  });
  await open(page);
  await page.getByRole("button", { name: "Collapse Work" }).click();
  await expect(item(page, "Planning")).toBeHidden();
  await page.reload();
  await expect(item(page, "Planning")).toBeVisible();
  await w.check();
});
