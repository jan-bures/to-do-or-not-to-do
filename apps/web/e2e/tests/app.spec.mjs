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

test("quick-add creates a destination on a fresh account and keeps focus", async ({ page }) => {
  const w = watch(page);
  await open(page, "/app?fixture=blank");
  const add = page.getByRole("textbox", { name: "Add a task" });
  await add.fill("Pay the invoice by fri !!");
  await add.press("Enter");
  await expect(page.getByRole("complementary", { name: "Lists" }).getByRole("link", { name: /^Tasks/ })).toBeVisible();
  await expect(row(page, "Pay the invoice").locator(".row-title")).toHaveText("Pay the invoice");
  await expect(add).toHaveValue("");
  await expect(add).toBeFocused();
  await w.check();
});

test("quick-add keeps ordinary trailing words and escaped syntax in the title", async ({ page }) => {
  await open(page, "/app?fixture=blank");
  const add = page.getByRole("textbox", { name: "Add a task" });
  for (const [input, title] of [
    ["Enjoy the sun", "Enjoy the sun"],
    ["Where the dog sat", "Where the dog sat"],
    ["Notes from the call yesterday", "Notes from the call yesterday"],
    ["Wow !", "Wow !"],
    ["Wow!", "Wow!"],
    [String.raw`See you \tomorrow`, "See you tomorrow"],
  ]) {
    await add.fill(input);
    await add.press("Enter");
    await expect(rows(page).getByText(title, { exact: true })).toBeVisible();
  }
});

test("quick-add previews only parsed dates and clears the chip after submit", async ({ page }) => {
  const w = watch(page);
  await open(page, "/app?fixture=blank");
  const add = page.getByRole("textbox", { name: "Add a task" });
  const preview = page.locator(".qa-date-chip");
  await add.fill("Water the plants by fri !!");
  await expect(preview).toHaveText("Fri 27 Nov");
  await expect(preview).toBeVisible();
  await add.fill("Water the plants 2027-01-04");
  await expect(preview).toHaveText("Mon 4 Jan 2027");
  await add.fill("Water the plants today");
  await expect(preview).toHaveText("Sat 21 Nov");
  await add.fill(String.raw`Water the plants \tomorrow`);
  await expect(preview).toBeHidden();
  await add.fill("Water the plants zítra !!");
  await expect(preview).toHaveText("Sun 22 Nov");
  await add.press("Enter");
  await expect(preview).toBeHidden();
  await expect(add).toBeFocused();
  await expect(row(page, "Water the plants").locator(".col-due")).toHaveText("Tomorrow");
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

test("exact undo restores a recurring task and removes its next occurrence", async ({ page }) => {
  const w = watch(page);
  await open(page);
  await page.locator(".sidebar").getByRole("link", { name: /^Planning/ }).click();
  const title = "Prepare the November board update";
  const original = await row(page, title).locator(".row-title").getAttribute("id");
  await row(page, title).locator(".check").click();
  await expect(page.locator(".toast")).toContainText(title);
  await expect(page.locator(".toast-undo")).toBeVisible();
  await expect(row(page, title)).toHaveCount(1, { timeout: 3000 });
  await expect(row(page, title).locator(".row-title")).not.toHaveAttribute("id", original);
  // The toast can close without losing the exact target. Ctrl+Z in an input is left to the browser.
  await page.locator(".toast-close").click();
  const add = page.getByRole("textbox", { name: "Add a task" });
  await add.fill("draft");
  await add.press("Control+z");
  await expect(row(page, title).locator(".row-title")).not.toHaveAttribute("id", original);
  await page.locator(".room-title").focus();
  await page.keyboard.press("Control+z");
  await expect(row(page, title)).toHaveCount(1);
  await expect(row(page, title).locator(".row-title")).toHaveAttribute("id", original);
  await expect(row(page, title).getByRole("checkbox")).not.toBeChecked();
  await w.check();
});

test("a conflicting exact undo gives a plain-language explanation", async ({ page }) => {
  const w = watch(page);
  await open(page);
  await page.locator(".sidebar").getByRole("link", { name: /^Planning/ }).click();
  const title = "Prepare the November board update";
  const original = await row(page, title).locator(".row-title").getAttribute("id");
  await row(page, title).getByRole("checkbox").click();
  await expect(row(page, title)).toHaveCount(1, { timeout: 3000 });
  await expect(row(page, title).locator(".row-title")).not.toHaveAttribute("id", original);
  await row(page, title).locator(".row-title").click();
  await page.keyboard.press("e");
  const editor = page.getByRole("textbox", { name: "Title", exact: true });
  await editor.fill("Work on the next board meeting");
  await editor.press("Enter");
  await expect(row(page, "Work on the next board meeting")).toHaveCount(1);
  await page.locator(".room-title").focus();
  await page.keyboard.press("Control+z");
  await expect(page.locator(".toast")).toContainText("Couldn't undo: the item was changed again.");
  await expect(row(page, "Work on the next board meeting")).toHaveCount(1);
  await expect(row(page, title)).toHaveCount(0);
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

test("the due date accepts relative and past dates, rejects bad input, and can be cleared", async ({ page }) => {
  const w = watch(page);
  await open(page);
  const board = row(page, "Prepare the November board update");
  await board.locator(".row-title").click();
  const input = page.getByRole("textbox", { name: "Due date" });
  const due = page.getByRole("button", { name: /^Edit due date/ });
  await page.keyboard.press("d");
  await expect(input).toBeFocused();
  await expect(input).toHaveValue("2026-11-21");
  await input.fill("next week maybe");
  await input.press("Enter");
  await expect(input).toHaveAttribute("aria-invalid", "true");
  await expect(input).toHaveValue("next week maybe");
  await input.press("Escape");
  await expect(due).toHaveText("Today");
  await due.click();
  await input.fill("zítra");
  await input.press("Enter");
  await expect(due).toHaveText("Tomorrow");
  await expect(board.locator(".col-due")).toHaveText("Tomorrow");
  await due.click();
  await input.fill("2026-11-20");
  await input.press("Enter");
  await expect(board.locator(".col-due")).toHaveText("1d overdue");
  await due.click();
  await input.fill("");
  await input.press("Enter");
  await expect(due).toHaveText("Set date");
  await expect(board.locator(".col-due")).toBeEmpty();
  await expect(page.getByRole("button", { name: "Repeat", exact: true })).toHaveText("Does not repeat");
  await w.check();
});

test("four priority levels stay in sync with the star, row and P shortcut", async ({ page }) => {
  const w = watch(page);
  await open(page);
  const task = row(page, "Review the API latency report");
  const title = task.locator(".row-title");
  await title.click();
  const priority = page.getByRole("combobox", { name: "Priority" });
  const star = page.getByRole("button", { name: "Important", exact: true });
  for (const [level, mark] of [["High", "!!!"], ["Low", "!"], ["Normal", "!!"], ["None", ""]]) {
    await priority.selectOption(level);
    await expect(priority).toHaveValue(level);
    await expect(task.locator(".col-priority")).toHaveText(mark);
    await expect(star).toHaveAttribute("aria-pressed", String(level === "High"));
  }
  await title.click();
  for (const level of ["Low", "Normal", "High", "None"]) {
    await page.keyboard.press("p");
    await expect(priority).toHaveValue(level);
  }
  await page.keyboard.press("Shift+p");
  await expect(priority).toBeFocused();
  await w.check();
});

test("two ArrowDown presses on the closed priority select keep focus and selection", async ({ page }) => {
  const w = watch(page);
  await open(page);
  const title = "Review the API latency report";
  await row(page, title).locator(".row-title").click();
  const priority = page.getByRole("combobox", { name: "Priority" });
  await expect(priority).toHaveValue("None");
  await priority.focus();
  await page.keyboard.press("ArrowDown");
  await expect(priority).toHaveValue("Low");
  await expect(priority).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(priority).toHaveValue("Normal");
  await expect(priority).toBeFocused();
  await expect(page.locator(".detail .d-title")).toHaveText(title);
  await expect(row(page, title)).toHaveClass(/selected/);
  await w.check();
});

test("holding P cycles the priority only once", async ({ page }) => {
  const w = watch(page);
  await open(page);
  const title = row(page, "Review the API latency report").locator(".row-title");
  await title.click();
  const priority = page.getByRole("combobox", { name: "Priority" });
  await expect(priority).toHaveValue("None");
  await page.keyboard.down("p");
  await expect(priority).toHaveValue("Low");
  await page.keyboard.down("p");
  await page.keyboard.down("p");
  await page.keyboard.up("p");
  await expect(priority).toHaveValue("Low");
  await page.keyboard.press("p");
  await expect(priority).toHaveValue("Normal");
  await w.check();
});

test("task shortcuts stay out of the quick-add and due-date inputs", async ({ page }) => {
  const w = watch(page);
  await open(page);
  await page.locator(".sidebar").getByRole("link", { name: /^Team/ }).click();
  const titles = rows(page).locator(".row-title");
  const order = await titles.allTextContents();
  // Pick a middle task so both reorder directions would have an effect.
  await titles.nth(1).click();
  const detail = page.locator(".detail .d-title");
  const priority = page.getByRole("combobox", { name: "Priority" });
  const dueButton = page.getByRole("button", { name: /^Edit due date/ });
  const dueInput = page.getByRole("textbox", { name: "Due date" });
  const add = page.getByRole("textbox", { name: "Add a task" });
  for (const input of [add, dueInput]) {
    if (input === add) await add.focus();
    else await dueButton.click();
    await expect(input).toBeFocused();
    await input.fill("");
    for (const key of ["p", "d", "Space", "Alt+ArrowUp", "Alt+ArrowDown"]) {
      await page.keyboard.press(key);
      await expect(input).toBeFocused();
      await expect(detail).toHaveText(order[1]);
      await expect(titles).toHaveText(order);
      await expect(priority).toHaveValue("Low");
      if (input === add) await expect(dueInput).toHaveCount(0);
    }
    await expect(input).toHaveValue("pd ");
  }
  await dueInput.press("Escape");
  await expect(dueButton).toHaveText("Set date");
  await w.check();
});

test("step titles can be renamed, reordered and restored after deletion", async ({ page }) => {
  const w = watch(page);
  await open(page);
  await row(page, "Prepare the November board update").locator(".row-title").click();
  const steps = page.locator(".step-rows .step");
  const title = steps.first().locator(".step-title");
  const original = await title.textContent();
  await title.click();
  const input = page.getByRole("textbox", { name: "Step title" });
  await expect(input).toBeFocused();
  await input.fill("Discard this draft");
  await input.press("Escape");
  await expect(title).toHaveText(original);
  await title.click();
  await input.fill("Check the first draft");
  await input.press("Enter");
  await expect(title).toHaveText("Check the first draft");
  await expect(steps.first().getByRole("button", { name: "Move step 1 up" })).toBeDisabled();
  await steps.first().getByRole("button", { name: "Move step 1 down" }).click();
  await expect(steps.nth(1).locator(".step-title")).toHaveText("Check the first draft");
  await expect(steps.nth(1).getByRole("button", { name: "Move step 2 down" })).toBeFocused();
  await steps.nth(1).getByRole("button", { name: "Move step 2 up" }).click();
  await expect(title).toHaveText("Check the first draft");
  await expect(steps.first().getByRole("button", { name: "Move step 1 up" })).toBeFocused();
  const remove = steps.first().getByRole("button", { name: "Delete step 1" });
  await remove.click();
  await expect(steps).toHaveCount(4);
  await expect(page.locator(".step-title", { hasText: "Check the first draft" })).toHaveCount(0);
  await expect(page.getByRole("textbox", { name: "Add step" })).toBeFocused();
  const toast = page.locator(".toast");
  await expect(toast).toContainText("Deleted step “Check the first draft”");
  await toast.getByRole("button", { name: /Undo/ }).click();
  await expect(steps).toHaveCount(5);
  await expect(title).toHaveText("Check the first draft");
  await expect(steps.first().getByRole("checkbox")).toBeChecked();
  await remove.click();
  await expect(steps).toHaveCount(4);
  await expect(page.getByRole("textbox", { name: "Add step" })).toBeFocused();
  await toast.getByRole("button", { name: /Undo/ }).focus();
  await page.keyboard.press("Control+z");
  await expect(title).toHaveText("Check the first draft");
  await w.check();
});

test("Alt+Up and Alt+Down reorder in a concrete list, not in My Day", async ({ page }) => {
  const w = watch(page);
  await open(page);
  const latency = "Review the API latency report";
  const agenda = "Draft the agenda for Tuesday’s sync";
  await row(page, latency).locator(".row-title").click();
  await page.keyboard.press("Alt+ArrowDown");
  await page.locator(".sidebar").getByRole("link", { name: /^Team/ }).click();
  const titles = rows(page).locator(".row-title");
  const order = await titles.allTextContents();
  expect(order.slice(0, 2)).toEqual([latency, agenda]);
  await titles.first().click();
  await page.keyboard.press("Alt+ArrowUp");
  await expect(titles).toHaveText(order);
  await page.keyboard.down("Alt");
  await page.keyboard.down("ArrowDown");
  await page.keyboard.down("ArrowDown"); // held key, not a second move
  await page.keyboard.up("ArrowDown");
  await page.keyboard.up("Alt");
  await expect(titles).toHaveText([agenda, latency, ...order.slice(2)]);
  await expect(row(page, latency)).toHaveClass(/selected/);
  await page.locator(".toast-undo").click();
  await expect(titles).toHaveText(order);
  await page.keyboard.press("Alt+ArrowDown");
  await page.keyboard.press("Alt+ArrowUp");
  await expect(titles).toHaveText(order);
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
  await expect(page.locator(".detail .d-title")).toBeHidden();
  await expect(rows(page).first().locator(".row-title")).toBeFocused();
  const backup = row(page, "Rotate the backup encryption keys");
  await backup.locator(".row-title").click();
  await page.keyboard.press("Space");
  await expect(backup).toHaveClass(/completing/);
  await page.keyboard.press("Control+z");
  await expect(backup).not.toHaveClass(/completing/);
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

test("the medium detail dims and disables the room, traps focus, and restores the opener", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 1024, height: 768 });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await open(page);
  const title = rows(page).first().locator(".row-title");
  const scrim = page.locator(".scrim");
  const detail = page.getByRole("dialog", { name: "Task detail" });
  await title.click();
  await expect(detail).toBeFocused();
  await expect(detail).toHaveAttribute("aria-modal", "true");
  await expect(page.locator("main")).toHaveAttribute("inert", "");
  await expect(scrim).toHaveCSS("opacity", "1");
  await expect(scrim).not.toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await expect(detail).toHaveCSS("transform", "none");
  const sun = detail.getByRole("button", { name: "My Day" });
  await page.keyboard.press("Tab");
  await expect(sun).toBeFocused();
  const last = detail.getByRole("button", { name: "Add file" });
  await last.focus();
  // Keydown with repeat=true must still wrap at the last control.
  await page.keyboard.down("Tab");
  await expect(sun).toBeFocused();
  await last.focus();
  await page.keyboard.down("Tab");
  await page.keyboard.up("Tab");
  await expect(sun).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(last).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(detail).toBeHidden();
  await expect(title).toBeFocused();
  await expect(page.locator("main")).not.toHaveAttribute("inert");
  await title.press("Enter");
  await expect(detail).toBeFocused();
  await scrim.click({ position: { x: 400, y: 280 } });
  await expect(detail).toBeHidden();
  await expect(title).toBeFocused();
  await w.check();
});

test("the completed-task toast stays announced and usable in medium detail", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 1024, height: 768 });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await open(page);
  const task = row(page, "Rotate the backup encryption keys");
  const detail = page.getByRole("dialog", { name: "Task detail" });
  const toast = detail.locator(".toast");
  const announcement = detail.locator(".detail-toast").getByRole("status");
  const undo = toast.getByRole("button", { name: /Undo/ });
  const openDetail = () => row(page, "Review the API latency report").locator(".row-title").click();
  const complete = () => task.getByRole("checkbox").click();

  await complete();
  await openDetail();
  await expect(page.locator("main")).toHaveAttribute("inert", "");
  await expect(announcement).toHaveAttribute("aria-live", "polite");
  await expect(announcement).toContainText("Completed “Rotate the backup encryption keys”");
  await expect(detail).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(toast.getByRole("button", { name: "Dismiss" })).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(undo).toBeFocused();
  await undo.click();
  await expect(task.locator(".check")).toHaveAttribute("aria-checked", "false");
  await expect(announcement).toContainText("Change undone.");
  await expect(undo).toHaveCount(0);
  await expect(detail).toBeFocused();

  // Completing a task from inside the modal uses the already-mounted live region.
  await detail.getByRole("checkbox", { name: "Complete", exact: true }).click();
  await expect(announcement).toContainText("Completed “Review the API latency report”");
  await undo.click();
  await expect(detail.getByRole("checkbox", { name: "Complete", exact: true })).toBeVisible();
  await w.check();
});

test("filter: slash and Ctrl+K focus it, with core queries and Czech text in the current room", async ({ page }) => {
  const w = watch(page);
  await open(page);
  await page.keyboard.press("/");
  const filter = page.getByRole("searchbox", { name: "Filter tasks" });
  await expect(filter).toBeFocused();
  await filter.fill("tag:docs");
  await expect(rows(page)).toHaveCount(1);
  await expect(row(page, "Prepare the November board update")).toBeVisible();
  await expect(page.locator(".summary")).toContainText("3 of 12 done");
  await filter.fill("KVULI STK");
  await expect(rows(page)).toHaveCount(1);
  await expect(row(page, "Zavolat do servisu kvůli STK")).toBeVisible();
  await filter.fill("standing desk");
  await expect(rows(page)).toHaveCount(0);
  await expect(page.locator(".empty-line")).toHaveText("Nothing matches the filter.");
  await filter.press("Escape");
  await expect(page.getByRole("button", { name: "Filter tasks" })).toBeFocused();
  await expect(rows(page)).toHaveCount(9);
  await page.keyboard.press("Control+k");
  await expect(filter).toBeFocused();
  await filter.fill("done ondrej");
  await expect(page.getByRole("button", { name: "Completed 1" })).toHaveAttribute("aria-expanded", "false");
  await page.getByRole("button", { name: "Completed 1" }).click();
  await expect(page.getByRole("list", { name: "Completed tasks" }).getByRole("button", { name: "Reply to Ondřej about the offsite" })).toBeVisible();
  await w.check();
});

test("collection disclosure preserves navigation and new list creates in the current collection", async ({ page }) => {
  const w = watch(page);
  await open(page);
  const sidebar = page.getByRole("complementary", { name: "Lists" });
  await expect(sidebar.getByRole("region", { name: "Agents" })).toContainText("Codex");
  const work = sidebar.getByRole("link", { name: /^Work/ });
  const finance = sidebar.getByRole("link", { name: /^Finance/ });
  await finance.click();
  const toggle = sidebar.getByRole("button", { name: "Collapse Work" });
  await toggle.press("Enter");
  await expect(finance).toBeHidden();
  await expect(work).toHaveClass(/contains-active/);
  await expect(page.locator(".room-title")).toHaveText("Finance");
  const newList = sidebar.getByRole("button", { name: "New list" });
  const name = sidebar.getByRole("textbox", { name: "List name" });
  await newList.click();
  await expect(name).toBeFocused();
  await name.press("Escape");
  await expect(newList).toBeFocused();
  await newList.click();
  await expect(name).toBeFocused();
  await name.fill("Reading for the project");
  await name.press("Enter");
  await expect(page.locator(".room-title")).toHaveText("Reading for the project");
  await expect(page.locator(".eyebrow-lead")).toHaveText("Work");
  await expect(sidebar.getByRole("link", { name: "Reading for the project" })).toHaveAttribute("aria-current", "page");
  await expect(finance).toBeVisible();
  const add = page.getByRole("textbox", { name: "Add a task" });
  await add.fill("File a receipt");
  await add.press("Enter");
  await add.fill("Pay the invoice by fri !!");
  await add.press("Enter");
  await expect(rows(page).first().locator(".row-title")).toHaveText("Pay the invoice");
  await expect(rows(page).first().locator(".col-priority")).toHaveText("!!");
  await expect(rows(page).nth(1).locator(".row-title")).toHaveText("File a receipt");
  await rows(page).first().locator(".row-title").click();
  await expect(page.locator(".props .prop").first()).toContainText("Fri 27 Nov");
  await w.check();
});

test("finished My Day keeps quick-add and makes today's completed tasks available", async ({ page }) => {
  const w = watch(page);
  await open(page, "/app?fixture=finished");
  await expect(page.locator(".room-head.finished")).toBeVisible();
  await expect(page.locator(".finished .summary")).toHaveText(/12 of 12 done\s*·\s*all clear/);
  await expect(page.getByRole("link", { name: "Plan Sunday" })).toBeVisible();
  const completed = page.getByRole("button", { name: "Completed 12" });
  await expect(completed).toHaveAttribute("aria-expanded", "false");
  await completed.click();
  const done = page.getByRole("list", { name: "Completed tasks" });
  await expect(done.getByRole("listitem")).toHaveCount(12);
  await completed.click();
  await expect(done).toBeHidden();
  const add = page.getByRole("textbox", { name: "Add a task" });
  await add.fill("One more thing today");
  await add.press("Enter");
  await expect(add).toBeFocused();
  await expect(page.locator(".room-head.finished")).toBeHidden();
  await expect(row(page, "One more thing").locator(".row-title")).toHaveText("One more thing");
  await w.check();
});

test("hand-over rows select, complete with choreography, and can be undone", async ({ page }) => {
  const w = watch(page);
  await open(page, "/app?fixture=finished");
  const handover = page.locator(".tomorrow");
  const lunch = "Připravit nedělní oběd";
  const detail = page.locator(".detail .d-title");
  await handover.getByRole("button", { name: lunch }).click();
  await expect(detail).toHaveText(lunch);
  await expect(page).toHaveURL(/\/app\/task\//);
  await page.keyboard.press("j");
  await expect(detail).toHaveText("Prepare the November board update");
  await page.keyboard.press("k");
  await expect(detail).toHaveText(lunch);
  await page.keyboard.press("Escape");
  await expect(handover.getByRole("button", { name: lunch })).toBeFocused();
  await handover.getByRole("checkbox", { name: `Complete “${lunch}”` }).click();
  await expect(handover.getByRole("checkbox", { name: `Reopen “${lunch}”` })).toHaveAttribute("aria-checked", "true");
  await expect(page.locator(".room-head.finished")).toBeVisible();
  await expect(page.locator(".finished .summary")).toContainText("12 of 12 done");
  await expect(handover.getByRole("button", { name: lunch })).toHaveCount(0);
  await page.keyboard.press("Control+z");
  await expect(handover.getByRole("checkbox", { name: `Complete “${lunch}”` })).toBeVisible();
  await handover.getByRole("button", { name: "Submit the October expense report" }).click();
  await expect(detail).toHaveText("Submit the October expense report");
  await page.keyboard.press("Space");
  await expect(handover.getByRole("checkbox", { name: /Reopen.*October expense/ })).toHaveAttribute("aria-checked", "true");
  await expect(handover.getByRole("button", { name: "Submit the October expense report" })).toHaveCount(0);
  await w.check();
});

test("double-clicking a recurring hand-over checkbox spawns one occurrence, then exact Undo restores the original", async ({ page }) => {
  const w = watch(page);
  await open(page, "/app?fixture=finished");
  const handover = page.locator(".tomorrow");
  const title = "Prepare the November board update";
  await handover.getByRole("checkbox", { name: `Complete “${title}”` }).dblclick();
  await expect(handover.getByRole("checkbox", { name: `Reopen “${title}”` })).toBeDisabled();
  const toast = page.locator(".toast");
  await expect(toast).toContainText(`Completed “${title}”`);
  await expect(toast.getByRole("button", { name: /Undo/ })).toBeVisible();
  await expect(handover.getByRole("button", { name: title })).toHaveCount(0);
  await page.locator(".sidebar").getByRole("link", { name: /^All/ }).click();
  await expect(row(page, title)).toHaveCount(1);
  await expect(row(page, title).getByRole("checkbox")).not.toBeChecked();
  await expect(row(page, title).locator(".col-due")).toHaveText("Mon 30 Nov");
  await page.keyboard.press("Control+z");
  await expect(row(page, title)).toHaveCount(1);
  await expect(row(page, title).locator(".col-due")).toHaveText("Mon 23 Nov");
  await w.check();
});

test("finished My Day still opens Planned when tomorrow has no tasks", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 412, height: 920 });
  await open(page, "/app?fixture=finished-no-tomorrow");
  await expect(page.getByText("Nothing is due on Sunday yet.")).toBeVisible();
  await expect(page.locator(".finished .summary")).toHaveText(/12 of 12 done\s*·\s*all clear/);
  await expect(page.getByRole("textbox", { name: "Add a task" })).toBeVisible();
  await page.getByRole("link", { name: "Plan Sunday" }).click();
  await expect(page).toHaveURL(/\/app\/planned\?/);
  await expect(page.locator(".room-title")).toHaveText("Planned");
  await w.check();
});

test("phone columns align across mixed rows and disappear only when unused in the view", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 412, height: 920 });
  await open(page);
  const dated = row(page, "Prepare the November board update");
  const undated = row(page, "Review the API latency report");
  const left = (row, sel) => row.locator(sel).evaluate((e) => e.getBoundingClientRect().left);
  await expect(dated.locator(".due-slot")).toHaveCSS("width", "100px");
  await expect(undated.locator(".due-slot")).toHaveCSS("width", "100px");
  expect(await left(dated, ".chip")).toBeCloseTo(await left(undated, ".chip"));
  expect(await left(dated, ".col-agent")).toBeCloseTo(await left(undated, ".col-agent"));
  expect(await left(dated, ".col-priority")).toBeCloseTo(await left(undated, ".col-priority"));
  await page.getByRole("button", { name: "Filter tasks" }).click();
  const filter = page.getByRole("searchbox");
  await filter.fill("latency report");
  await expect(rows(page)).toHaveCount(1);
  await expect(undated.locator(".due-slot")).toBeHidden();
  await expect(undated.locator(".col-priority")).toBeHidden();
  await expect(undated.locator(".col-agent")).toBeVisible();
  expect(await left(undated, ".chip")).toBeCloseTo(await left(undated, ".row-title"));
  await w.check();
});

test("phone metadata ends in an ellipsis before columns, keeping the email and step count visible", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 320, height: 720 });
  await open(page);
  const board = row(page, "Prepare the November board update");
  const tags = board.locator(".tag");
  await expect(tags).toHaveCSS("text-overflow", "ellipsis");
  const fit = await board.locator(".meta").evaluate((meta) => {
    const tag = meta.querySelector(".tag");
    const chip = meta.querySelector(".chip-label");
    const edge = meta.getBoundingClientRect().right;
    return {
      tagsTruncated: tag.scrollWidth > tag.clientWidth,
      chipTruncated: chip.scrollWidth > chip.clientWidth,
      tagFits: tag.getBoundingClientRect().right <= edge + 1 && tag.clientWidth > 0,
      indicatorsFit: [...meta.querySelectorAll(".steps, .meta-icon")].every((el) => el.getBoundingClientRect().right <= edge),
    };
  });
  expect(fit).toEqual({ tagsTruncated: true, chipTruncated: true, tagFits: true, indicatorsFit: true });
  await expect(board.locator(".meta-icon + .tag")).toBeVisible();
  // The scrollport ends before the separate toast strip, so the last row can scroll fully into view.
  const bottom = rows(page).last();
  await bottom.scrollIntoViewIfNeeded();
  const y = await bottom.evaluate((el) => el.getBoundingClientRect().bottom);
  expect(y).toBeLessThanOrEqual(await page.locator(".list-foot").evaluate((el) => el.getBoundingClientRect().top));
  await w.check();
});

test("detail actions, schedule fields, sync label and context keyboard hints match their roles", async ({ page }) => {
  const w = watch(page);
  await open(page);
  const keys = page.locator(".sb-keys");
  await expect(keys).not.toContainText("Space");
  await expect(page.getByText(/^Synced /)).toHaveCount(1);
  await expect(page.locator(".sb-counts")).toHaveText("9 open 3 done");
  await rows(page).first().locator(".row-title").click();
  const buttons = page.locator(".detail-top button");
  expect(await buttons.evaluateAll((els) => els.map((el) => el.getAttribute("aria-label")))).toEqual(["My Day", "Important", "Close"]);
  const fields = page.getByRole("region", { name: "Schedule and priority" });
  await expect(fields.locator("dt")).toHaveText(["Due", "Priority", "Repeat", "Tags", "Reminder"]);
  await expect(fields.getByRole("button", { name: "Repeat" })).toBeEnabled();
  await expect(fields.getByRole("button", { name: "Tags" })).toBeEnabled();
  const reminder = fields.getByRole("button", { name: "Reminder", exact: true });
  await expect(reminder).toBeEnabled();
  await expect(reminder).toHaveAccessibleDescription("Today, 15:30");
  for (const value of [fields.getByRole("button", { name: /^Edit due date:/ }), reminder.getByText("Today, 15:30")]) {
    await expect(value).toHaveCSS("font-family", /Geist Mono/);
    await expect(value).toHaveCSS("font-size", "12.5px");
    await expect(value).toHaveCSS("font-weight", "500");
  }
  await expect(keys).toContainText("Esc");
  await expect(keys).toContainText("Space");
  await page.locator(".sidebar").getByRole("link", { name: "Garden" }).click();
  await expect(page.locator(".empty-line")).toHaveText("Nothing on the shelf yet.");
  await expect(keys).toHaveText("N add task");
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

test("the tab, the home screen and the sidebar show the Daybreak app icon", async ({ page }) => {
  const w = watch(page);
  await open(page);
  // The links point at the generated assets, each served with its type.
  const brand = new URL("../../../../assets/brand/", import.meta.url);
  const expected = {
    "icon-small.svg": [new URL("small-16.svg", brand), "image/svg+xml"],
    "icon-16.png": [new URL("generated/icon-16.png", brand), "image/png"],
    "icon-32.png": [new URL("generated/icon-32.png", brand), "image/png"],
    "apple-touch-icon.png": [new URL("generated/apple-touch-icon.png", brand), "image/png"],
  };
  const hrefs = await page
    .locator('link[rel="icon"], link[rel="apple-touch-icon"]')
    .evaluateAll((ls) => ls.map((l) => l.getAttribute("href")));
  expect(hrefs.sort()).toEqual(Object.keys(expected).map((f) => `/app/brand/${f}`).sort());
  const { readFileSync } = await import("node:fs");
  for (const [file, [source, type]] of Object.entries(expected)) {
    const res = await page.request.get(`/app/brand/${file}`);
    expect(res.status(), file).toBe(200);
    expect(res.headers()["content-type"], file).toContain(type);
    expect(Buffer.compare(await res.body(), readFileSync(source)), file).toBe(0);
  }
  // The sidebar's mark is the same 16 px drawing, loaded under the CSP.
  const mark = page.locator(".sidebar .brand-mark");
  await expect(mark).toHaveAttribute("src", "/app/brand/icon-small.svg");
  await expect.poll(() => mark.evaluate((img) => img.complete && img.naturalWidth > 0)).toBe(true);
  await w.check();
});
