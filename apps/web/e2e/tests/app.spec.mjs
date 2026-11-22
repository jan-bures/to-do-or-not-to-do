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

test("completing a recurring task offers no Undo, so no extra occurrence can be left behind", async ({ page }) => {
  const w = watch(page);
  await open(page);
  // Weekly, with a due date: completing it schedules the next one, which reopening would not remove.
  const title = "Prepare the November board update";
  await expect(row(page, title)).toHaveCount(1);
  await row(page, title).locator(".check").click();
  await expect(page.locator(".toast")).toContainText(title);
  await expect(page.locator(".toast")).toContainText("next one is scheduled");
  await expect(page.locator(".toast-undo")).toHaveCount(0);
  // Ctrl+Z does nothing either: the task leaves My Day completed.
  await page.keyboard.press("Control+z");
  await expect(row(page, title)).toHaveCount(0, { timeout: 3000 });
  await expect(page.locator(".summary")).toContainText("4 of 12 done");
  // Its list has exactly one open copy: the next occurrence.
  await page.locator(".sidebar").getByRole("link", { name: /^Planning/ }).click();
  await expect(row(page, title)).toHaveCount(1);
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
  const last = detail.getByRole("button", { name: /board-pack-draft/ });
  await page.keyboard.press("Shift+Tab");
  await expect(last).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(sun).toBeFocused();
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
  const undo = toast.getByRole("button", { name: /Undo/ });
  const openDetail = () => row(page, "Review the API latency report").locator(".row-title").click();
  const complete = () => task.getByRole("checkbox").click();

  await complete();
  await openDetail();
  await expect(page.locator("main")).toHaveAttribute("inert", "");
  await expect(detail.getByRole("status")).toHaveAttribute("aria-live", "polite");
  await expect(detail.getByRole("status")).toContainText("Completed “Rotate the backup encryption keys”");
  await expect(detail).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(toast.getByRole("button", { name: "Dismiss" })).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(undo).toBeFocused();
  await undo.click();
  await expect(task.locator(".check")).toHaveAttribute("aria-checked", "false");
  await expect(toast).toHaveCount(0);
  await expect(detail).toBeFocused();

  // Completing a task from inside the modal uses the already-mounted live region.
  await detail.getByRole("checkbox", { name: "Complete", exact: true }).click();
  await expect(detail.getByRole("status")).toContainText("Completed “Review the API latency report”");
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

test("detail actions, read-only fields, sync label and context keyboard hints match their roles", async ({ page }) => {
  const w = watch(page);
  await open(page);
  const keys = page.locator(".sb-keys");
  await expect(keys).not.toContainText("Space");
  await expect(page.getByText(/^Synced /)).toHaveCount(1);
  await expect(page.locator(".sb-counts")).toHaveText("9 open 3 done");
  await rows(page).first().locator(".row-title").click();
  const buttons = page.locator(".detail-top button");
  expect(await buttons.evaluateAll((els) => els.map((el) => el.getAttribute("aria-label")))).toEqual(["My Day", "Important", "Close"]);
  const fields = page.getByRole("region", { name: "Read-only task fields" });
  await expect(fields.locator("dt")).toHaveText(["Due", "Reminder", "Repeat", "Tags"]);
  const muted = await page.locator(".detail .notes").evaluate((el) => getComputedStyle(el).color);
  await expect(fields.locator("dd").first()).toHaveCSS("color", muted);
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
