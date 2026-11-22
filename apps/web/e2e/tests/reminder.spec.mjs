import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";

test.use({ viewport: { width: 1440, height: 900 } });

const task = (page, name = "Review the API latency report") => page.getByRole("button", { name, exact: true });
const trigger = (page) => page.getByRole("button", { name: "Reminder", exact: true });
const presets = (page) => page.getByRole("dialog", { name: "Reminder", exact: true });
const custom = (page) => page.getByRole("dialog", { name: "Custom reminder", exact: true });
const undo = (page) => page.locator(".toast").getByRole("button", { name: /^Undo/ });

test("reminder presets are keyboard accessible and adding or removing a reminder is undoable", async ({ page }) => {
  const w = watch(page);
  await page.emulateMedia({ reducedMotion: "reduce" });
  await open(page);
  await task(page).click();
  const reminder = trigger(page);
  await expect(reminder).toHaveAccessibleDescription("Set reminder");
  await expect(reminder.getByText("Set reminder")).toHaveCSS("font-family", /^"?Geist"?,/);
  await reminder.press("Enter");
  await expect(reminder).toHaveAttribute("aria-expanded", "true");
  const reducedFade = await page.locator("html").evaluate((root) =>
    parseFloat(getComputedStyle(root).getPropertyValue("--duration-reduced-fade")) / 1000
  );
  expect(reducedFade).toBeGreaterThan(0);
  await expect(presets(page)).toHaveCSS("transition-duration", `${reducedFade}s`);
  await expect(presets(page)).toHaveCSS("transform", "none");
  const inAnHour = presets(page).getByRole("option", { name: "In 1 hour" });
  await expect(inAnHour).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(inAnHour).toBeFocused();
  await page.keyboard.press("ArrowDown");
  const tomorrow = presets(page).getByRole("option", { name: "Tomorrow at 09:00" });
  await expect(tomorrow).toBeFocused();
  await expect(reminder).toHaveText("Set reminder");
  await page.keyboard.press("Space");
  await expect(reminder).toBeFocused();
  await expect(reminder).toHaveText("Tomorrow, 09:00");
  await expect(page.locator(".toast")).toContainText("Reminder set for Tomorrow, 09:00");
  await expect(reminder).toHaveAttribute("aria-expanded", "false");
  await reminder.press("Enter");
  await expect(tomorrow).toHaveAttribute("aria-selected", "true");
  await expect(tomorrow.locator(".picker-tick")).toBeVisible();
  await expect(tomorrow).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(reminder).toBeFocused();
  await expect(presets(page)).toBeHidden();
  await reminder.press("Control+z");
  await expect(reminder).toHaveText("Set reminder");

  await reminder.press("Enter");
  await inAnHour.press("Enter");
  await expect(reminder).toHaveText("Today, 11:35");
  await reminder.press("Enter");
  await expect(presets(page).getByRole("option", { name: "Custom…", selected: true })).toBeFocused();
  await page.keyboard.press("Tab");
  const remove = presets(page).getByRole("button", { name: "Remove reminder" });
  await expect(remove).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(reminder).toBeFocused();
  await expect(reminder).toHaveText("Set reminder");
  await undo(page).click();
  await expect(reminder).toHaveText("Today, 11:35");
  await w.check();
});

test("custom reminder pre-fills the shown time, validates input and discards a cancelled draft", async ({ page }) => {
  const w = watch(page);
  await open(page);
  await task(page, "Prepare the November board update").click();
  const reminder = trigger(page);
  const chooseCustom = presets(page).getByRole("option", { name: "Custom…" });
  await expect(reminder).toHaveText("Today, 15:30");
  await reminder.click();
  await expect(chooseCustom).toBeFocused();
  await chooseCustom.press("Enter");
  const date = custom(page).getByLabel("Date", { exact: true });
  const time = custom(page).getByLabel("Time", { exact: true });
  const save = custom(page).getByRole("button", { name: "Save reminder" });
  const status = custom(page).getByRole("status");
  await expect(date).toBeFocused();
  await expect(date).toHaveValue("2026-11-21");
  await expect(time).toHaveValue("15:30");
  await page.keyboard.press("Shift+Tab");
  await expect(save).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(date).toBeFocused();
  // The native date input has several keyboard-focusable segments.
  for (let i = 0; i < 4 && !(await time.evaluate((el) => el === document.activeElement)); i++) {
    await page.keyboard.press("Tab");
  }
  await expect(time).toBeFocused();
  await time.fill("");
  await expect(save).toBeDisabled();
  await expect(status).toHaveText("Choose a date and time.");
  await time.fill("09:30");
  await expect(save).toBeDisabled();
  await expect(status).toHaveText("Pick a time that hasn’t passed.");
  await date.fill("2026-11-23");
  await expect(save).toBeEnabled();
  await custom(page).getByRole("button", { name: "Back", exact: true }).click();
  await expect(chooseCustom).toBeFocused();
  await chooseCustom.press("Enter");
  await expect(date).toHaveValue("2026-11-21");
  await date.fill("2026-11-23");
  await date.press("Escape");
  await expect(reminder).toHaveText("Today, 15:30");
  await expect(reminder).toBeFocused();

  await reminder.press("Enter");
  await chooseCustom.press("Enter");
  await save.click(); // The unchanged draft should not enqueue a change or offer Undo.
  await expect(reminder).toBeFocused();
  await expect(undo(page)).toHaveCount(0);
  await reminder.press("Enter");
  await chooseCustom.press("Enter");
  await date.fill("2026-11-23");
  await time.fill("14:20");
  await save.press("Enter");
  await expect(reminder).toBeFocused();
  await expect(reminder).toHaveText("Mon 23 Nov, 14:20");
  await undo(page).click();
  await expect(reminder).toHaveText("Today, 15:30");
  await w.check();
});

test("a read-only tab still shows the reminder but cannot open the editor", async ({ page }) => {
  const w = watch(page);
  await open(page, "/app?fixture=read-only");
  await task(page, "Prepare the November board update").click();
  await expect(trigger(page)).toHaveText("Today, 15:30");
  await expect(trigger(page)).toBeDisabled();
  await expect(presets(page)).toBeHidden();
  await w.check();
});
