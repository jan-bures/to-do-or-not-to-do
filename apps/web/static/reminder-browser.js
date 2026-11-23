// Browser APIs that can be missing or throw even if the app itself works.
let notificationsUnavailable = false;

export function reminderPermission() {
  try {
    if (notificationsUnavailable || typeof Notification !== "function") return "unsupported";
    // An embedding/browser policy may expose the constructor but not the prompt.
    const permission = Notification.permission;
    return permission === "default" && typeof Notification.requestPermission !== "function"
      ? "unsupported"
      : permission;
  } catch {
    return "unsupported";
  }
}

// Must be called synchronously by the Settings button's click handler.
export async function requestReminderPermission() {
  try {
    if (reminderPermission() === "default") await Notification.requestPermission();
  } catch {
    // For example, the browser disallows permission prompts in this context.
    notificationsUnavailable = true;
  }
}

export function showReminderNotification(title, task, open) {
  try {
    if (reminderPermission() !== "granted") return false;
    // No body, task ID, list name or other task details go to the system.
    const notification = new Notification(title);
    notification.onclick = (event) => {
      event.preventDefault();
      try { window.focus(); } catch { /* The browser can refuse a focus request. */ }
      notification.close();
      open(task);
    };
    return true;
  } catch {
    // Some mobile browsers expose Notification, but only support a service worker's API.
    if (reminderPermission() === "granted") notificationsUnavailable = true;
    return false;
  }
}

// Claims contain only opaque reminder IDs and times. The BroadcastChannel is an optimisation:
// for a simultaneous alert, only one readwrite transaction can record the claim, and a reload
// sees the committed record even if it missed the broadcast. No localStorage check/set race.
const storeName = "fired";
const seen = new Map();
let channel;
let opening;

function openClaims() {
  if (!opening) {
    opening = new Promise((resolve, reject) => {
      const request = indexedDB.open("todo-reminder-alerts", 1);
      request.onupgradeneeded = () => {
        request.result.createObjectStore(storeName);
      };
      request.onsuccess = () => {
        const db = request.result;
        db.onversionchange = () => { db.close(); opening = undefined; };
        resolve(db);
      };
      request.onerror = () => reject(request.error);
    }).catch((error) => {
      opening = undefined;
      throw error;
    });
  }
  return opening;
}

function listen() {
  if (channel !== undefined) return;
  try {
    channel = new BroadcastChannel("todo-reminder-alerts");
    channel.onmessage = ({ data }) => {
      if (typeof data?.key === "string" && Number.isFinite(data.at)) seen.set(data.key, data.at);
    };
  } catch {
    channel = null;
  }
}

// 1: this tab won, 0: already fired in another tab or before a reload, -1: no longer eligible.
// Reject if storage failed; retry later rather than deliver duplicates without coordination.
export async function claimReminder(key, at, cutoff, eligible) {
  listen();
  for (const [id, time] of seen) if (time < cutoff) seen.delete(id);
  if (seen.has(key)) return 0;
  const db = await openClaims();
  if (!eligible()) return -1;

  const claim = await new Promise((resolve, reject) => {
    const tx = db.transaction(storeName, "readwrite");
    const store = tx.objectStore(storeName);
    let result = -1;
    tx.onabort = () => reject(tx.error);
    tx.oncomplete = () => resolve(result);
    const id = [at, key];
    const lookup = store.getKey(id);
    lookup.onsuccess = () => {
      if (lookup.result !== undefined) {
        result = 0;
      } else if (eligible()) {
        store.put(null, id);
        result = 1;
      }
    };
    // The store is keyed by [millisecond, occurrence]; remove expired keys in this transaction.
    store.delete(IDBKeyRange.upperBound([cutoff], true));
  });
  if (claim >= 0) seen.set(key, at);
  if (claim === 1) {
    try { channel?.postMessage({ key, at }); } catch { /* The claim is already durable. */ }
  }
  return claim;
}
