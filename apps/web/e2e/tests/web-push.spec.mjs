import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";
import { mockPush } from "./web-push-helpers.mjs";
import { beforeReminder, reminderToast, REMINDER_TASK } from "./reminder-helpers.mjs";

const settings = (page) => page.getByRole("button", { name: "Settings", exact: true });
const status = (page) => page.locator("#reminder-notification-description");
const item = (page, name) => page.getByRole("menuitem", { name, exact: true });

test("Web Push opts in on a trusted gesture, sends a test, disables and unsubscribes on sign-out", async ({ page }) => {
  const w = watch(page);
  const { calls, endpoint } = await mockPush(page, "default");
  await open(page);
  await settings(page).click();
  await expect(status(page)).toContainText("Off.");
  const enable = item(page, "Turn on background reminders");
  await enable.dispatchEvent("click");
  expect(await page.evaluate(() => window.pushMock.activation)).toEqual([]);
  await enable.press("Enter");
  await expect(status(page)).toContainText("On.");
  expect(await page.evaluate(() => window.pushMock.activation)).toEqual([[true, true]]);
  expect(calls.find(c => c.method === "PUT")).toMatchObject({
    body: { endpoint, keys: { auth: "fixture", p256dh: "fixture" } }, csrf: "gallery-csrf",
  });
  await page.keyboard.press("ArrowDown");
  await expect(item(page, "Send test notification")).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.getByText("Test sent.")).toBeVisible();
  expect(calls.find(c => c.path.endsWith("/test"))).toMatchObject({ body: { endpoint }, method: "POST" });
  await item(page, "Turn off background reminders").click();
  await expect(status(page)).toContainText("Off.");
  expect(calls.some(c => c.method === "DELETE")).toBe(true);
  expect(await page.evaluate(() => window.pushMock.unsubscribed)).toBe(true);
  await enable.click();
  await expect(status(page)).toContainText("On.");
  await page.evaluate(() => { window.pushMock.unsubscribed = false; });
  await page.route("**/auth/logout", route => route.fulfill({ status: 204 }));
  const logout = page.waitForRequest("**/auth/logout");
  await item(page, "Sign out").click();
  await logout;
  expect(await page.evaluate(() => window.pushMock.unsubscribed)).toBe(true);
  await w.check();
});

test("the worker notifies even when the tab won the claim, then opens the task without reloading", async ({ page }) => {
  const w = watch(page);
  const { calls, endpoint } = await mockPush(page, "granted", true);
  await beforeReminder(page);
  await settings(page).click();
  await page.clock.runFor(32);
  await expect(status(page)).toContainText("On.");
  expect(calls.find(c => c.path.endsWith("/status"))).toMatchObject({
    body: { endpoint, keys: { auth: "fixture", p256dh: "fixture" } },
  });
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: REMINDER_TASK, exact: true }).click();
  const task = new URL(page.url()).pathname.split("/").at(-1);
  expect(task).toMatch(/^[a-f0-9-]{36}$/);
  await page.goBack();
  await expect(page).not.toHaveURL(/\/task\//);
  // Load the worker module in an isolated page context with worker event and notification shims.
  // Unlike a vendor push service, this works offline and still exercises the real IndexedDB claim.
  await page.evaluate(async () => {
    const log = window.workerMock = { notifications: [], actions: [], windows: true };
    self.skipWaiting = async () => log.actions.push("skip waiting");
    self.registration = { showNotification: async (title, options) => log.notifications.push({ title, options }) };
    self.clients = {
      claim: async () => log.actions.push("claim clients"),
      matchAll: async () => log.windows ? [{
        url: location.href,
        focus: async () => log.actions.push("focused"),
        postMessage: data => {
          log.actions.push(data);
          const source = { scriptURL: new URL("/app/reminder-worker.js", location.origin).href };
          navigator.serviceWorker.dispatchEvent(Object.assign(new Event("message"), { data, source, origin: location.origin }));
        },
      }] : [],
      openWindow: async path => log.actions.push(path),
    };
    await import("/app/reminder-worker.js");
    window.workerEvent = (type, fields) => new Promise(resolve => {
      self.dispatchEvent(Object.assign(new Event(type), fields, { waitUntil: resolve }));
    });
  });
  const at = Date.parse("2026-11-21T14:30:00Z");
  await page.clock.runFor(18000);
  // The visible tab reached the fallback first and committed the claim.
  await expect(reminderToast(page)).toContainText(REMINDER_TASK);
  await page.evaluate(async ({ task, at, title }) => {
    await window.workerEvent("install", {});
    await window.workerEvent("activate", {});
    await window.workerEvent("push", { data: { json: () => ({ task, at, title }) } });
    await window.workerEvent("push", { data: { json: () => ({ task, at }) } });
    await window.workerEvent("push", { data: { json: () => { throw new SyntaxError(); } } });
    window.clickReminder = () => window.workerEvent("notificationclick", { notification: {
      data: { task }, close: () => window.workerMock.actions.push("closed"),
    } });
    await window.clickReminder();
  }, { task, at, title: REMINDER_TASK });
  expect(await page.evaluate(() => window.workerMock.notifications.map(({ title, options }) => [title, options.tag, options.renotify]))).toEqual([
    [REMINDER_TASK, `todo-reminder:${task}@${at}`, false],
    ["Reminder", `todo-reminder:${task}@${at}`, false],
    ["Reminder", "todo-reminder", false],
  ]);
  await expect(page.locator(".detail .d-title")).toHaveText(REMINDER_TASK);
  await expect(page).toHaveURL(new RegExp(`/app/list/[^/]+/task/${task}`));
  // These shims would have disappeared if notificationclick had reloaded the app.
  expect(await page.evaluate(() => window.workerMock.actions)).toEqual([
    "skip waiting", "claim clients", "closed", "focused", { type: "todo-open-reminder", task },
  ]);
  await page.evaluate(async () => { window.workerMock.windows = false; await window.clickReminder(); });
  expect(await page.evaluate(() => window.workerMock.actions.at(-1))).toBe(`/app/all/task/${task}`);
  await w.check();
});

test("only reminder-worker messages from this origin with UUID task IDs are forwarded", async ({ page, context }) => {
  await open(page);
  const task = "01234567-89ab-4cde-8fab-0123456789ab";
  const data = { type: "todo-open-reminder", task };
  const workerStarted = context.waitForEvent("serviceworker");
  await page.evaluate(async data => {
    const { onReminderOpen } = await import("/app/reminder-browser.js");
    window.openedReminders = [];
    onReminderOpen(task => window.openedReminders.push(task));
    const scriptURL = `${location.origin}/app/reminder-worker.js`;
    const message = (fields) => navigator.serviceWorker.dispatchEvent(Object.assign(new Event("message"), {
      origin: location.origin, source: { scriptURL }, data, ...fields,
    }));
    message({ origin: "https://example.com" });
    message({ source: { scriptURL: `${location.origin}/app/other-worker.js` } });
    message({ data: { ...data, task: "not-a-uuid" } });
    await navigator.serviceWorker.register(scriptURL, { scope: "/app", type: "module" });
    await navigator.serviceWorker.ready;
  }, data);
  expect(await page.evaluate(() => window.openedReminders)).toEqual([]);
  // Check that the browser supplies the expected origin and source for a real worker message.
  const worker = await workerStarted;
  await worker.evaluate(async data => {
    for (const client of await self.clients.matchAll({ type: "window", includeUncontrolled: true })) {
      client.postMessage(data);
    }
  }, data);
  await expect.poll(() => page.evaluate(() => window.openedReminders)).toEqual([task]);
});

test("the reminder worker installs at the app scope without prompting", async ({ page }) => {
  const w = watch(page);
  await page.route("**/api/v1/web-push", route => route.fulfill({ json: { public_key: `B${"A".repeat(86)}` } }));
  await page.addInitScript(() => {
    // Headless Chromium can report a permanent denial even when this context grants permission.
    Object.defineProperty(Notification, "permission", { get: () => "default" });
    window.permissionRequests = 0;
    Notification.requestPermission = async () => { window.permissionRequests += 1; return "granted"; };
  });
  await open(page);
  await settings(page).click();
  await expect(status(page)).toContainText("Off.");
  expect(await page.evaluate(async () => {
    const app = await navigator.serviceWorker.getRegistration("/app");
    return [new URL(app.scope).pathname, new URL(app.active.scriptURL).pathname, Boolean(await navigator.serviceWorker.getRegistration("/admin"))];
  })).toEqual(["/app", "/app/reminder-worker.js", false]);
  expect(await page.evaluate(() => window.permissionRequests)).toBe(0);
  await w.check();
});
