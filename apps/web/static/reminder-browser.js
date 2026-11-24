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

export function showReminderNotification(title, task, key, open) {
  try {
    if (reminderPermission() !== "granted") return false;
    // No body, list name or notes go to the system. The opaque tag matches Web Push.
    const notification = new Notification(title, { tag: reminderTag(key), renotify: false });
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

// Claims contain only opaque task IDs and times. The BroadcastChannel is an optimisation:
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
export function reminderKey(task, at) {
  return `${task}@${at}`;
}

export function reminderTag(key) {
  return `todo-reminder:${key}`;
}

export function validReminderTask(task) {
  return typeof task === "string" && /^[\da-f]{8}-[\da-f]{4}-[\da-f]{4}-[\da-f]{4}-[\da-f]{12}$/i.test(task);
}

export function onReminderOpen(open) {
  navigator.serviceWorker?.addEventListener?.("message", ({ data, source, origin }) => {
    if (origin === location.origin
        && source?.scriptURL === new URL("/app/reminder-worker.js", location.origin).href
        && data?.type === "todo-open-reminder" && validReminderTask(data.task)) {
      open(data.task);
    }
  });
}

export async function claimReminder(key, at, cutoff, eligible, legacy) {
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
        const finish = (alreadyFired) => {
          if (!eligible()) return;
          store.put(null, id);
          result = alreadyFired ? 0 : 1;
        };
        if (legacy) {
          // Preserve alerts already fired by builds that keyed claims by reminder ID.
          const previous = store.getKey([at, legacy]);
          previous.onsuccess = () => finish(previous.result !== undefined);
        } else {
          finish(false);
        }
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

const api = "/api/v1/web-push";
const fetchOptions = { credentials: "same-origin", cache: "no-store", redirect: "error" };
let prepared;
let signingOut = false;
let pendingEnable;

async function sessionInfo() {
  const response = await fetch("/auth/session", fetchOptions);
  if (!response.ok) throw new Error("No live session");
  return response.json();
}

async function pushRequest(path, method, body) {
  const meta = (name) => document.querySelector(`meta[name="${name}"]`)?.content;
  const request = (csrf) => fetch(`${api}${path}`, {
    ...fetchOptions, method,
    headers: { "X-CSRF-Token": csrf, ...(body && { "Content-Type": "application/json" }) },
    body: body && JSON.stringify(body),
  });
  const sent = meta("csrf-token");
  if (!sent) throw new Error("No live session");
  const response = await request(sent);
  if (response.status !== 403) return response;
  const current = await sessionInfo();
  return current.owner_scope === meta("todo-owner-scope") && current.csrf !== sent
    ? request(current.csrf) : response;
}

function decodeKey(value) {
  return Uint8Array.from(atob(value.replaceAll("-", "+").replaceAll("_", "/")), c => c.charCodeAt(0));
}

function matchesKey(subscription, key) {
  const stored = new Uint8Array(subscription.options.applicationServerKey);
  return key.length === stored.length && key.every((byte, index) => byte === stored[index]);
}

async function activeRegistration() {
  const registered = await navigator.serviceWorker.register("/app/reminder-worker.js", {
    scope: "/app", type: "module", updateViaCache: "none",
  });
  if (registered.active) return registered;
  let timer;
  try {
    return await Promise.race([
      navigator.serviceWorker.ready,
      new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("Worker not active")), 5000); }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

export async function webPushState() {
  prepared = undefined;
  try {
    if (signingOut || !("serviceWorker" in navigator) || typeof PushManager !== "function"
        || typeof Notification !== "function") return "unavailable";
    const response = await pushRequest("", "GET");
    if (!response.ok || response.status === 204) return "unavailable";
    if (Notification.permission === "denied") return "blocked";
    const key = decodeKey((await response.json()).public_key);
    const registration = await activeRegistration();
    let subscription = await registration.pushManager.getSubscription();
    if (subscription) {
      const { endpoint, keys } = subscription.toJSON();
      const status = matchesKey(subscription, key) && await pushRequest("/status", "POST", { endpoint, keys });
      if (!status || (status.ok && !(await status.json()).active)) {
        // Revocation, inactivity, or a rotated key requires opting in again.
        await subscription.unsubscribe();
        subscription = null;
      } else if (!status.ok) {
        return "unavailable";
      }
    }
    if (signingOut) return "unavailable";
    prepared = { key, registration, subscription };
    return subscription ? "on" : "off";
  } catch {
    return "unavailable";
  }
}

// Invoke subscribe before the first await: on mobile (notably Safari) the browser requires that
// the permission prompt come directly from a click or key activation. Preparation never prompts.
export function enableWebPush() {
  if (signingOut || !prepared) return Promise.resolve("error");
  if (pendingEnable) return pendingEnable;
  const { key, registration } = prepared;
  pendingEnable = (async () => {
    let subscription;
    try {
      subscription = await registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: key });
      if (signingOut) return "error";
      const { endpoint, keys } = subscription.toJSON();
      if (!(await pushRequest("", "PUT", { endpoint, keys })).ok) return "error";
      return "ok";
    } catch {
      return Notification.permission === "denied" ? "blocked" : "error";
    } finally {
      if (signingOut && subscription) await subscription.unsubscribe().catch(() => {});
    }
  })().finally(() => { pendingEnable = undefined; });
  return pendingEnable;
}

export async function disableWebPush() {
  try {
    const subscription = prepared?.subscription;
    if (!subscription) return "ok";
    const response = await pushRequest("", "DELETE", { endpoint: subscription.endpoint });
    if (!response.ok) return "error";
    await subscription.unsubscribe();
    return "ok";
  } catch {
    return "error";
  }
}

export async function testWebPush() {
  try {
    const subscription = prepared?.subscription;
    if (!subscription) return "error";
    const response = await pushRequest("/test", "POST", { endpoint: subscription.endpoint });
    if (response.status === 429) return "wait";
    if (response.status === 410) return "expired";
    return response.ok ? "ok" : "error";
  } catch {
    return "error";
  }
}

export async function unsubscribeForSignOut() {
  signingOut = true;
  prepared = undefined;
  let timer;
  try {
    await Promise.race([
      (async () => {
        const registration = await navigator.serviceWorker?.getRegistration("/app");
        const subscription = await registration?.pushManager?.getSubscription();
        await subscription?.unsubscribe();
      })(),
      new Promise(resolve => { timer = setTimeout(resolve, 750); }),
    ]);
  } catch { /* Logout still revokes the server registration. */ }
  finally { clearTimeout(timer); }
}
