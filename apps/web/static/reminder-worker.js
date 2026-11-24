// No fetch handler or offline cache. This worker only handles reminder notifications and clicks.
import { claimReminder, reminderKey, sessionInfo } from "./reminder-browser.js";

const uuid = /^[\da-f]{8}-[\da-f]{4}-[\da-f]{4}-[\da-f]{4}-[\da-f]{12}$/i;
const icon = "/app/brand/icon-192.png";
const appPath = (task) => typeof task === "string" && uuid.test(task) ? `/app/all/task/${task}` : "/app";

async function title(task) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 4000);
  try {
    const { csrf } = await sessionInfo(controller.signal);
    const response = await fetch(`/api/v1/web-push/tasks/${task}`, {
      credentials: "same-origin", cache: "no-store", redirect: "error",
      headers: { "X-CSRF-Token": csrf }, signal: controller.signal,
    });
    if (response.ok) {
      const { title } = await response.json();
      if (typeof title === "string" && title.trim()) return title;
    }
  } catch { /* Session expired, task unavailable, or temporarily offline. */ }
  finally { clearTimeout(timer); }
  return "Reminder";
}

async function deliver(payload) {
  if (payload === null) {
    await self.registration.showNotification("Test notification", { tag: "todo-test-reminder", icon });
    return;
  }
  const { task, at } = payload ?? {};
  if (typeof task !== "string" || !uuid.test(task) || !Number.isSafeInteger(at)) return;
  const key = reminderKey(task, at);
  // Share a durable claim with the in-tab fallback. A tag also coalesces repeated delivery if
  // storage is temporarily unavailable; do not drop the user's only alert in that case.
  const claimed = await claimReminder(key, at, Date.now() - 86400000, () => true).catch(() => 1);
  if (claimed !== 1) return;
  await self.registration.showNotification(await title(task), { tag: `todo-reminder:${key}`, icon, data: { task } });
}

self.addEventListener("push", (event) => {
  try {
    if (event.data) event.waitUntil(deliver(event.data.json()));
  } catch { /* Ignore a malformed message. */ }
});

async function openTask(task) {
  const url = appPath(task);
  const windows = (await self.clients.matchAll({ type: "window", includeUncontrolled: true })).filter(client => {
    const address = new URL(client.url);
    return address.origin === self.location.origin && (address.pathname === "/app" || address.pathname.startsWith("/app/"));
  });
  const existing = windows.find(client => client.focused) ?? windows[0];
  if (existing) {
    try {
      const client = task ? await existing.navigate(url) : existing;
      if (client) {
        await client.focus();
        return;
      }
    } catch { /* The app tab may have closed since matchAll. */ }
  }
  await self.clients.openWindow(url);
}

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  event.waitUntil(openTask(event.notification.data?.task));
});
