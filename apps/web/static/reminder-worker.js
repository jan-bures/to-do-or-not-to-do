// No fetch handler or offline cache. This worker only handles reminder notifications and clicks.
import { claimReminder, reminderKey, reminderTag, validReminderTask } from "./reminder-browser.js";

const icon = "/app/brand/icon-192.png";

self.addEventListener("install", (event) => event.waitUntil(self.skipWaiting()));
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

async function deliver(payload) {
  if (payload === null) {
    await self.registration.showNotification("Test notification", { tag: "todo-test-reminder", icon, renotify: false });
    return;
  }
  const { task, at, title } = payload ?? {};
  if (!validReminderTask(task) || !Number.isSafeInteger(at)) {
    await self.registration.showNotification("Reminder", { tag: "todo-reminder", icon, renotify: false });
    return;
  }
  const key = reminderKey(task, at);
  // Record delivery for the in-tab fallback. WebKit still requires a visible notification for
  // every push, even if the tab already won the claim or storage failed. Use its same tag.
  const claim = claimReminder(key, at, Date.now() - 86400000, () => true).catch(() => {});
  const notification = self.registration.showNotification(
    typeof title === "string" && title.trim() ? title : "Reminder",
    { tag: reminderTag(key), icon, renotify: false, data: { task } },
  );
  await Promise.all([claim, notification]);
}

self.addEventListener("push", (event) => {
  let payload;
  try { payload = event.data?.json(); } catch { /* Still show a generic notification. */ }
  event.waitUntil(deliver(payload));
});

async function openTask(task) {
  const windows = (await self.clients.matchAll({ type: "window", includeUncontrolled: true })).filter(client => {
    const address = new URL(client.url);
    return address.origin === self.location.origin && (address.pathname === "/app" || address.pathname.startsWith("/app/"));
  });
  // matchAll orders windows by most recently focused, even if none are currently focused.
  if (!windows.length) {
    await self.clients.openWindow(validReminderTask(task) ? `/app/all/task/${task}` : "/app");
    return;
  }
  for (const client of windows) {
    try {
      await client.focus();
    } catch { /* The browser can refuse focus. Still send the route if the tab is open. */ }
    try {
      if (validReminderTask(task)) client.postMessage({ type: "todo-open-reminder", task });
      return;
    } catch { /* The app tab may have closed since matchAll. */ }
  }
}

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  event.waitUntil(openTask(event.notification.data?.task));
});
