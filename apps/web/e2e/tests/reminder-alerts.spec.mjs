import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";
import { beforeReminder, mockNotifications, reminderToast, REMINDER_PATH, REMINDER_TASK } from "./reminder-helpers.mjs";

const settings = (page) => page.getByRole("button", { name: "Settings", exact: true });
const permissions = (page) => page.locator("#reminder-notification-description");
const shown = (page) => page.evaluate(() => window.reminderMock.notifications.map(({ title }) => title));

test("permission is requested only on activation, and the title-only alert opens and focuses its task", async ({ page }) => {
  const w = watch(page);
  await mockNotifications(page, "default");
  await beforeReminder(page);
  expect(await page.evaluate(() => window.reminderMock.requests)).toEqual([]);
  await settings(page).press("Enter");
  await page.clock.runFor(32);
  await expect(permissions(page)).toContainText("Not enabled");
  expect(await page.evaluate(() => window.reminderMock.requests)).toEqual([]);
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  const enable = page.getByRole("menuitem", { name: "Enable reminder notifications" });
  await expect(enable).toBeFocused();
  await enable.dispatchEvent("click");
  expect(await page.evaluate(() => window.reminderMock.requests)).toEqual([]);
  await page.keyboard.press("Enter");
  await expect(permissions(page)).toContainText("Allowed");
  expect(await page.evaluate(() => window.reminderMock.requests)).toEqual([true]);
  await page.keyboard.press("Escape");

  await page.clock.runFor(9946); // two milliseconds before the due instant
  expect(await shown(page)).toEqual([]);
  await page.clock.runFor(2);
  await expect.poll(() => shown(page)).toEqual([REMINDER_TASK]);
  await expect(reminderToast(page)).toHaveCount(0);
  expect(await page.evaluate(() => {
    const notice = window.reminderMock.notifications[0];
    if (notice.options !== undefined) return false;
    notice.onclick(new Event("click", { cancelable: true }));
    return notice.closed && window.reminderMock.focused === 1;
  })).toBe(true);
  await expect(page.locator(".detail .d-title")).toHaveText(REMINDER_TASK);
  await expect(page).toHaveURL(/\/app\/list\/[^/]+\/task\/[^/?]+/);
  await w.check();
});

test("a simultaneous alert is delivered only once across tabs and reloads, even without a channel", async ({ page, context }) => {
  const other = await context.newPage();
  const monitors = [watch(page), watch(other)];
  await Promise.all([page, other].map((tab) => mockNotifications(tab, "granted", { noChannel: true })));
  await beforeReminder(page); // One clock for the browser context.
  await open(other, REMINDER_PATH);
  const allShown = async () => (await Promise.all([shown(page), shown(other)])).flat();
  await page.clock.runFor(10000);
  await expect.poll(allShown).toEqual([REMINDER_TASK]);
  await page.clock.runFor(30000);
  expect(await allShown()).toEqual([REMINDER_TASK]);
  await page.reload();
  await page.locator(".room-title").first().waitFor();
  await page.clock.runFor(30000);
  expect(await shown(page)).toEqual([]);
  for (const monitor of monitors) await monitor.check();
});

for (const permission of ["denied", "unsupported", "granted"]) {
  const cannotConstruct = permission === "granted";
  test(`an in-app alert when notifications are ${cannotConstruct ? "non-constructible" : permission}`, async ({ page }) => {
    const w = watch(page);
    await mockNotifications(page, permission, { cannotConstruct });
    await beforeReminder(page);
    await page.clock.runFor(10000);
    const toast = reminderToast(page);
    await expect(toast).toContainText(REMINDER_TASK);
    await settings(page).click();
    await expect(permissions(page)).toContainText(permission === "denied" ? "browser’s site settings" : "Not supported here");
    await page.keyboard.press("Escape");
    expect(await page.evaluate(() => window.reminderMock.requests)).toEqual([]);
    expect(await shown(page)).toEqual([]);
    await toast.getByRole("button", { name: "Snooze 10 min" }).click();
    await expect(page.locator(".toast-text")).toHaveText("Reminder snoozed for 10 min");
    await page.clock.fastForward(599999);
    await expect(toast).toHaveCount(0);
    await page.clock.fastForward(1);
    await expect(toast).toContainText(REMINDER_TASK);
    await toast.getByRole("button", { name: "Complete", exact: true }).click();
    await expect(page.locator(".toast-text")).toContainText(`Completed “${REMINDER_TASK}”`);
    await expect(toast).toHaveCount(0);
    await w.check();
  });
}

test("moving a reminder reschedules the timer and removing it cancels the alert", async ({ page }) => {
  const w = watch(page);
  await mockNotifications(page, "granted");
  await beforeReminder(page);
  await page.getByRole("button", { name: REMINDER_TASK, exact: true }).click();
  const reminder = page.getByRole("button", { name: "Reminder", exact: true });
  await reminder.click();
  const presets = page.getByRole("dialog", { name: "Reminder", exact: true });
  await presets.getByRole("option", { name: "In 1 hour" }).click();
  await page.clock.runFor(15000);
  expect(await shown(page)).toEqual([]);
  await reminder.click();
  await presets.getByRole("button", { name: "Remove reminder" }).click();
  await expect(reminder).toHaveText("Set reminder");
  await page.clock.fastForward(3600000);
  expect(await shown(page)).toEqual([]);
  await w.check();
});
