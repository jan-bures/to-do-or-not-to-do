// The real thing: sign in with a passkey (a virtual authenticator), then use the app against the server.
// Covers the session + CSRF path of the API, the event stream, edits that survive a reload while the
// server can't be reached (from one tab or several, sent by one tab in order), storage failures, and
// database upgrades that other tabs block.
//
// The sync tests share one browser context (one cookie jar, like the owner's browser) and run in
// order: the first one sets the server up. The clock test uses a separate replica.
import { expect, test } from "@playwright/test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { DATA, PORT } from "../live-env.mjs";
import { openDeviceSettings } from "../tests/helpers.mjs";

test.describe.configure({ mode: "serial" });

const baseURL = `http://localhost:${PORT}`;
const rows = (page) => page.locator(".rows:not(.completed-rows) .row");
const row = (page, title) => rows(page).filter({ hasText: title });

let context;
/** The passkey the setup registered, to sign in from other tabs. */
let credentials = [];

/** Fails on page errors and CSP violations. */
function watch(page) {
  const problems = [];
  page.on("pageerror", (e) => problems.push(e.message));
  page.on("console", (m) => {
    if (/Content.Security.Policy/i.test(m.text())) problems.push(m.text());
  });
  return problems;
}

/** A virtual authenticator for `page`, holding the registered passkey (if there is one yet). */
async function authenticator(page, ctx = context, seed = true) {
  const cdp = await ctx.newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  const { authenticatorId } = await cdp.send("WebAuthn.addVirtualAuthenticator", {
    options: {
      protocol: "ctap2",
      transport: "internal",
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
      automaticPresenceSimulation: true,
    },
  });
  for (const credential of seed ? credentials : []) {
    await cdp.send("WebAuthn.addCredential", { authenticatorId, credential });
  }
  return { cdp, authenticatorId };
}

/** The server's event log as `page`'s session sees it. */
async function serverLog(page) {
  return page.evaluate(async () => {
    const csrf = document.querySelector('meta[name="csrf-token"]').content;
    const res = await fetch("/api/v1/events?since=0&limit=500", {
      headers: { "X-CSRF-Token": csrf, accept: "application/json" },
    });
    return { status: res.status, body: await res.text() };
  });
}

/** How many times `text` occurs in the server's log. */
async function occurrences(page, text) {
  const { body } = await serverLog(page);
  return body.split(text).length - 1;
}

/** A tab on the app, synced. */
async function openApp() {
  const page = await context.newPage();
  await page.goto("/app");
  await expect(page.locator(".room-title")).toHaveText("My Day");
  await expect(page.locator(".sync-text")).toHaveAttribute("title", /^Synced/, { timeout: 15_000 });
  return page;
}

/** How many outbox records in IndexedDB mention `text` (what a closed tab would still send). */
async function stored(page, text) {
  return page.evaluate(
    (text) =>
      new Promise((resolve, reject) => {
        const open = indexedDB.open("todo-replica");
        open.onerror = () => reject(open.error);
        open.onsuccess = () => {
          const db = open.result;
          const req = db.transaction("outbox").objectStore("outbox").getAll();
          req.onsuccess = () => {
            db.close();
            resolve(req.result.filter((v) => String(v).includes(text)).length);
          };
          req.onerror = () => {
            db.close();
            reject(req.error);
          };
        };
      }),
    text,
  );
}

/** Counts the durably stored refusals, not a tab's in-memory replica. */
function failedCount(page) {
  return page.evaluate(() => new Promise((resolve, reject) => {
    const open = indexedDB.open("todo-replica");
    open.onerror = () => reject(open.error);
    open.onsuccess = () => {
      const db = open.result;
      const req = db.transaction("failed").objectStore("failed").count();
      req.onsuccess = () => { db.close(); resolve(req.result); };
      req.onerror = () => { db.close(); reject(req.error); };
    };
  }));
}

/** Waits until the edit mentioning `text` is committed to IndexedDB. */
async function committed(page, text) {
  await expect.poll(() => stored(page, text), { timeout: 15_000 }).toBeGreaterThan(0);
}

/** Can be toggled per tab to simulate a quota failure. */
function injectStorageFailure() {
  const put = IDBObjectStore.prototype.put;
  IDBObjectStore.prototype.put = function (...args) {
    if (window.__failIdb) throw new DOMException("injected write failure", "QuotaExceededError");
    return put.apply(this, args);
  };
}

/** Renames a task through the detail pane. */
async function rename(page, from, to) {
  await row(page, from).click();
  await expect(page.locator(".d-title")).toHaveText(from);
  await page.keyboard.press("e");
  const input = page.locator(".title-input");
  await expect(input).toBeFocused();
  await input.fill(to);
  await input.press("Enter");
  await expect(page.locator(".d-title")).toHaveText(to);
}

async function add(page, title) {
  await page.locator(".qa-input").fill(title);
  await page.locator(".qa-input").press("Enter");
  await expect(row(page, title)).toHaveCount(1);
}

const listLink = (page, name) => page.locator(".sidebar").getByRole("link", { name: new RegExp(`^${name}(?: |$)`) });
const taskButton = (page, title) => page.locator(".room").getByRole("button", { name: title, exact: true });
const movePicker = (page) => page.getByRole("dialog", { name: "Move to…" });

async function createRoom(page, kind, name) {
  const label = kind === "list" ? "List" : "Collection";
  await page.getByRole("button", { name: "New", exact: true }).click();
  await page.getByRole("menuitem", { name: label, exact: true }).click();
  const input = page.getByRole("textbox", { name: `${label} name` });
  await input.fill(name);
  await input.press("Enter");
  await expect(page.locator(".room-title")).toHaveText(name);
}

async function deleteRoom(page, kind) {
  await page.getByRole("button", { name: "Room menu" }).click();
  await page.getByRole("menuitem", { name: `Delete ${kind}`, exact: true }).click();
  await page.getByRole("dialog", { name: `Delete ${kind}` }).getByRole("button", { name: "Delete", exact: true }).click();
}

test.beforeAll(async ({ browser }) => {
  context = await browser.newContext({ baseURL, viewport: { width: 1440, height: 900 } });
});

test.afterAll(async () => {
  await context?.close();
});

test("set up, sign in, and use the app against the server", async () => {
  const page = await context.newPage();
  const problems = watch(page);
  const { cdp, authenticatorId } = await authenticator(page);

  // First run: the setup code from the data directory, then a passkey.
  await page.goto("/setup");
  // The file's first line is the code; the rest explains it.
  const code = readFileSync(join(DATA, "setup-code.txt"), "utf8").trim().split(/\s+/)[0];
  await page.locator('input[name="code"]').fill(code);
  await page.getByRole("button", { name: "Register passkey" }).click();
  await page.waitForURL((u) => !u.pathname.startsWith("/setup"));
  ({ credentials } = await cdp.send("WebAuthn.getCredentials", { authenticatorId }));
  expect(credentials.length).toBe(1);

  // The root opens the app.
  await page.goto("/");
  await expect(page).toHaveURL(/\/app$/);
  await expect(page.locator(".room-title")).toHaveText("My Day");
  await expect(page.locator(".sync-text")).toHaveAttribute("title", /^Synced/, { timeout: 15_000 });

  // Lists come from the server; there are none yet, so make one through the API with the session.
  const created = await page.evaluate(async () => {
    const csrf = document.querySelector('meta[name="csrf-token"]').content;
    const id = "0193b2c0-0000-7000-8000-000000000001";
    const res = await fetch("/api/v1/commands", {
      method: "POST",
      headers: { "X-CSRF-Token": csrf, "content-type": "application/json", accept: "application/json" },
      body: JSON.stringify({
        command_id: "0193b2c0-0000-7000-8000-0000000000aa",
        command: { cmd: "create_list", id, collection: null, name: "Inbox", color: "lagoon", placement: { at: "last" } },
      }),
    });
    return { status: res.status, body: await res.text() };
  });
  expect(created.status, created.body).toBeLessThan(300);
  // The event stream brings it in.
  await expect(page.locator(".sidebar")).toContainText("Inbox", { timeout: 15_000 });

  // Add a task online.
  await add(page, "Zalít květiny");
  await expect.poll(async () => (await serverLog(page)).body, { timeout: 15_000 }).toContain("Zalít květiny");

  // The server can't be reached: the edit waits in the outbox (IndexedDB).
  await page.route("**/api/v1/commands", (r) => r.abort("internetdisconnected"));
  await add(page, "Koupit mléko offline");
  await expect(page.locator(".sync-text")).not.toHaveAttribute("title", /^Synced/, { timeout: 15_000 });
  expect((await serverLog(page)).body).not.toContain("Koupit mléko offline");
  await committed(page, "Koupit mléko offline");

  // Reload with the server reachable again: the pending edit comes back from IndexedDB and is sent.
  await page.unroute("**/api/v1/commands");
  await page.reload();
  await expect(row(page, "Koupit mléko offline")).toHaveCount(1);
  await expect.poll(async () => (await serverLog(page)).body, { timeout: 15_000 }).toContain("Koupit mléko offline");
  await expect(page.locator(".sync-text")).toHaveAttribute("title", /^Synced/, { timeout: 15_000 });

  // Live updates: a change made in another tab shows up here.
  const other = await context.newPage();
  await other.goto("/app");
  await add(other, "Z druhé záložky");
  await expect(row(page, "Z druhé záložky")).toHaveCount(1, { timeout: 15_000 });
  await other.close();

  // Completing works against the server too.
  await row(page, "Zalít květiny").locator(".check").click();
  await expect(row(page, "Zalít květiny")).toHaveCount(0, { timeout: 5_000 });
  await expect.poll(async () => (await serverLog(page)).body, { timeout: 15_000 }).toContain('"field":"completed_at"');

  // A request without the CSRF header is refused.
  const bare = await page.evaluate(async () => (await fetch("/api/v1/events?since=0")).status);
  expect(bare).toBe(403);

  // Pages that carry the token are never stored or transformed on the way (BREACH).
  const shell = await page.request.get("/app");
  expect(shell.headers()["cache-control"]).toBe("no-store, no-transform");
  // The real server serves the app icon with its type.
  for (const [file, type] of [["icon-small.svg", "image/svg+xml"], ["icon-32.png", "image/png"], ["apple-touch-icon.png", "image/png"]]) {
    const res = await page.request.get(`/app/brand/${file}`);
    expect(res.status(), file).toBe(200);
    expect(res.headers()["content-type"], file).toContain(type);
  }

  expect(problems).toEqual([]);
  await page.close();
});

test("a task move, delete and both exact undos reach another tab", async () => {
  const page = await openApp();
  const problems = watch(page);
  const title = "Pack the exhibit labels";
  const task = (tab) => taskButton(tab, title);
  const undo = () => page.locator(".toast").getByRole("button", { name: "Undo" });

  await createRoom(page, "list", "Exhibit");
  await add(page, title);

  const observer = await openApp();
  const observerProblems = watch(observer);
  await listLink(observer, "Exhibit").click();
  await expect(task(observer)).toBeVisible();

  await task(page).click();
  await page.keyboard.press("m");
  await page.getByRole("dialog", { name: "Move to…" }).getByRole("option", { name: "Inbox" }).click();
  await expect(task(observer)).toHaveCount(0, { timeout: 15_000 });
  await listLink(observer, "Inbox").click();
  await expect(task(observer)).toBeVisible();
  await undo().click();
  await expect(task(observer)).toHaveCount(0, { timeout: 15_000 });
  await listLink(observer, "Exhibit").click();
  await expect(task(observer)).toBeVisible();

  await task(page).click();
  await page.getByRole("button", { name: "Task actions" }).click();
  await page.getByRole("menuitem", { name: "Delete task" }).click();
  await expect(task(observer)).toHaveCount(0, { timeout: 15_000 });
  await undo().click();
  await expect(task(observer)).toBeVisible();

  // Leave the shared server's top-level list order unchanged for the later sidebar tests.
  await deleteRoom(page, "list");
  await expect(listLink(observer, "Exhibit")).toHaveCount(0);
  expect([...problems, ...observerProblems]).toEqual([]);
  await observer.close();
  await page.close();
});

test("a source change on another tab invalidates the open move picker", async () => {
  const a = await openApp();
  const b = await openApp();
  const problems = [watch(a), watch(b)];
  for (const name of ["Move source", "Move second", "Move third"]) {
    await createRoom(b, "list", name);
  }
  const title = "Proof from the original list";
  await listLink(b, "Move source").click();
  await add(b, title);
  await listLink(a, "All").click();
  await taskButton(a, title).click();
  await a.keyboard.press("m");
  const filter = movePicker(a).getByRole("textbox", { name: "Filter lists" });
  await filter.fill("Move third");
  await filter.press("ArrowDown");
  await expect(movePicker(a).getByRole("option", { name: "Move third" })).toBeFocused();

  await taskButton(b, title).press("m");
  await movePicker(b).getByRole("option", { name: "Move second" }).click();
  await expect(a.locator(".crumb-list")).toHaveText("Move second");
  await expect(movePicker(a)).toBeHidden();
  await a.keyboard.press("Enter");
  await listLink(b, "Move third").click();
  await expect(taskButton(b, title)).toHaveCount(0);
  await listLink(b, "Move second").click();
  await expect(taskButton(b, title)).toBeVisible();

  for (const name of ["Move source", "Move second", "Move third"]) {
    await listLink(b, name).click();
    await deleteRoom(b, "list");
    await expect(listLink(a, name)).toHaveCount(0);
  }
  expect(problems.flat()).toEqual([]);
  await b.close();
  await a.close();
});

test("remote picker updates preserve option focus and removals focus the filter", async () => {
  const a = await openApp();
  const b = await openApp();
  const problems = [watch(a), watch(b)];
  await createRoom(b, "collection", "Picker destinations");
  await createRoom(b, "list", "Option one");
  await add(a, "Focus check task");
  const task = taskButton(a, "Focus check task");
  await task.press("m");
  const picker = movePicker(a);
  const filter = picker.getByRole("textbox", { name: "Filter lists" });
  await filter.fill("Option");
  await filter.press("ArrowDown");
  const group = picker.getByRole("group", { name: "Picker destinations" });
  await expect(group.getByRole("option", { name: "Option one" })).toBeFocused();

  await listLink(b, "Picker destinations").click();
  await createRoom(b, "list", "Option two");
  const second = group.getByRole("option", { name: "Option two" });
  await expect(second).toBeVisible();
  await expect(group.getByRole("option", { name: "Option one" })).toBeFocused();
  await a.keyboard.press("ArrowDown");
  await expect(second).toBeFocused();
  const options = group.getByRole("option");
  await expect(options).toHaveText(["Option one", "Option two"]);

  await b.getByRole("button", { name: "Room menu" }).click();
  await b.getByRole("menuitem", { name: "Move up" }).click();
  await expect(options).toHaveText(["Option two", "Option one"]);
  await expect(second).toBeFocused();

  // Reordering again must not pull the keyboard back from another control.
  const cancel = picker.getByRole("button", { name: "Cancel" });
  await cancel.focus();
  await b.getByRole("button", { name: "Room menu" }).click();
  await b.getByRole("menuitem", { name: "Move down" }).click();
  await expect(options).toHaveText(["Option one", "Option two"]);
  await expect(cancel).toBeFocused();
  await second.focus();

  await b.getByRole("button", { name: "Room menu" }).click();
  await b.getByRole("menuitem", { name: "Rename list" }).click();
  const renameInput = b.getByRole("dialog", { name: "Rename list" }).getByRole("textbox", { name: "List name" });
  await renameInput.fill("Option renamed");
  await renameInput.press("Enter");
  const renamed = group.getByRole("option", { name: "Option renamed" });
  await expect(renamed).toBeFocused();
  await deleteRoom(b, "list");
  await expect(renamed).toHaveCount(0);
  await expect(filter).toBeFocused();
  await filter.press("ArrowDown");
  await expect(group.getByRole("option", { name: "Option one" })).toBeFocused();
  await a.keyboard.press("Escape");
  await expect(task).toBeFocused();
  await a.keyboard.press("Delete");
  await expect(task).toHaveCount(0);
  await listLink(b, "Picker destinations").click();
  await deleteRoom(b, "collection");
  await expect(listLink(a, "Picker destinations")).toHaveCount(0);
  expect(problems.flat()).toEqual([]);
  await b.close();
  await a.close();
});

test("a signed-in tab invites a new browser without sharing its existing passkey", async ({ browser }, testInfo) => {
  const issuer = await openApp();
  const freshBrowser = await browser.newContext({ baseURL });
  try {
    const fresh = await freshBrowser.newPage();
    const { cdp, authenticatorId } = await authenticator(fresh, freshBrowser, false);
    const problems = watch(fresh);
    await openDeviceSettings(issuer);
    await issuer.getByRole("button", { name: "Pair a new device" }).click();
    await issuer.getByRole("button", { name: "Create invite" }).click();
    const link = issuer.getByRole("textbox", { name: "Invite link" });
    await expect(link).toBeVisible();
    const url = await link.inputValue();
    expect(new URL(url).hash).toMatch(/^#[A-Za-z0-9_-]{43}$/);
    await fresh.goto(url);
    await expect(fresh).toHaveURL(/\/pair\/invite$/);
    await fresh.getByRole("textbox", { name: "Device name" }).fill("Invited browser");
    await fresh.getByRole("button", { name: "Register passkey" }).click();
    await expect(fresh).toHaveURL(/\/app$/);
    await expect(fresh.locator(".sync-text")).toHaveAttribute("title", /^Synced/, { timeout: 15_000 });
    expect((await cdp.send("WebAuthn.getCredentials", { authenticatorId })).credentials).toHaveLength(1);
    await expect(issuer.getByText("Invited browser signed in.")).toBeVisible({ timeout: 10_000 });
    await fresh.goto("/admin/devices");
    const row = (page) => page.getByRole("row").filter({ hasText: "Invited browser" });
    await expect(row(fresh)).toBeVisible();
    await expect(row(fresh).getByRole("button", { name: "Revoke" })).toHaveCount(0);
    await issuer.goto("/admin/devices");
    await expect(row(issuer).getByRole("button", { name: "Revoke" })).toBeVisible();
    await expect(row(issuer).getByText("Also removes: 1 browser session.")).toBeVisible();
    await issuer.screenshot({ path: testInfo.outputPath("passkey-revocation.png") });
    expect(problems).toEqual([]);
  } finally {
    await freshBrowser.close();
    await issuer.close();
  }
});

test("an invite hides its credential while a status response is held past expiry", async () => {
  const page = await openApp();
  const id = "A".repeat(22);
  let release;
  let polled;
  const held = new Promise((resolve) => { release = resolve; });
  const pollStarted = new Promise((resolve) => { polled = resolve; });
  await page.route("**/api/v1/pair/invites", (route) => route.fulfill({ json: {
    id, code: "ABCDEF-GHJKMN", expires_in: 2,
    link: `${baseURL}/pair/invite#${"A".repeat(43)}`,
  } }));
  await page.route(`**/api/v1/pair/invites/${id}`, async (route) => {
    polled();
    await held;
    await route.fulfill({ json: { status: "used", device_name: "Last-second phone" } });
  });
  try {
    await openDeviceSettings(page);
    await page.getByRole("button", { name: "Pair a new device" }).click();
    await page.getByRole("button", { name: "Create invite" }).click();
    const qr = page.getByRole("img", { name: "Invite QR code" });
    await expect(qr).toBeVisible();
    await pollStarted;
    await expect(page.getByText("Checking whether the device joined…")).toBeVisible();
    await expect(qr).toHaveCount(0);
    await expect(page.getByRole("textbox", { name: "Invite link" })).toHaveCount(0);
    await expect(page.getByText("ABCDEF-GHJKMN", { exact: true })).toHaveCount(0);
    release();
    await expect(page.getByText("Last-second phone signed in.")).toBeVisible();
  } finally {
    release();
    await page.close();
  }
});

test("a stalled invite status cannot block a replacement after the grace period", async () => {
  const page = await openApp();
  const first = "A".repeat(22);
  const second = "B".repeat(22);
  let requests = 0;
  let release;
  let polled;
  let answered;
  const held = new Promise((resolve) => { release = resolve; });
  const pollStarted = new Promise((resolve) => { polled = resolve; });
  const oldReply = new Promise((resolve) => { answered = resolve; });
  await page.clock.install();
  await page.route("**/api/v1/pair/invites", (route) => route.fulfill({ json: {
    id: ++requests === 1 ? first : second, code: "ABCDEF-GHJKMN", expires_in: 2,
    link: `${baseURL}/pair/invite#${(requests === 1 ? "A" : "B").repeat(43)}`,
  } }));
  await page.route(`**/api/v1/pair/invites/${first}`, async (route) => {
    polled();
    await held;
    await route.fulfill({ json: { status: "used", device_name: "Old reply" } }).finally(answered);
  });
  await page.route(`**/api/v1/pair/invites/${second}`, (route) => route.fulfill({ json: { status: "pending" } }));
  const settings = async () => {
    await openDeviceSettings(page);
    await page.getByRole("button", { name: "Pair a new device" }).click();
  };
  const create = page.getByRole("button", { name: "Create invite" });
  try {
    await settings();
    await create.click();
    await pollStarted;
    await page.clock.fastForward(2_000);
    await expect(page.getByText("Checking whether the device joined…")).toBeVisible();
    await page.getByRole("button", { name: "Close", exact: true }).click();
    await page.clock.fastForward(60_000);
    await settings();
    await expect(page.getByText("Couldn’t confirm whether the device joined. Check Devices.")).toBeVisible();
    await create.click();
    const link = page.getByRole("textbox", { name: "Invite link" });
    await expect(link).toHaveValue(`${baseURL}/pair/invite#${"B".repeat(43)}`);
    release();
    await oldReply;
    await expect(link).toBeVisible();
    await expect(page.getByText("Old reply signed in.")).toHaveCount(0);
  } finally {
    release();
    await page.close();
  }
});

test("Activity reads confirmed changes and compensates another tab's past edit", async () => {
  const author = await openApp();
  const reader = await openApp();
  const problems = [author, reader].map(watch);
  const original = "Activity test original title";
  const edited = "Activity test changed title";
  await add(author, original);
  await expect(row(reader, original)).toBeVisible();
  await rename(author, original, edited);
  await expect(row(reader, edited)).toBeVisible();
  await expect.poll(() => occurrences(reader, edited)).toBe(1);

  await reader.getByRole("link", { name: "Activity", exact: true }).click();
  await expect(reader).toHaveURL(/\/app\/activity$/);
  const editRow = (page, title) => page.locator(".activity-row").filter({
    has: page.getByText(`Updated task "${title}" (title)`, { exact: true }),
  });
  const edit = editRow(reader, edited);
  await expect(edit).toBeVisible();
  await expect(edit.locator(".activity-meta")).toContainText("You");

  // Hold the outbox entry so a tab already open and a tab reloading both see the pending undo.
  const observer = await openApp();
  problems.push(watch(observer));
  await observer.getByRole("link", { name: "Activity", exact: true }).click();
  await expect(editRow(observer, edited).getByRole("button", { name: /^Undo:/ })).toBeEnabled();
  const undoPath = "**/api/v1/events/*/undo";
  await context.route(undoPath, (r) => r.abort("internetdisconnected"));
  await edit.getByRole("button", { name: /^Undo:/ }).click();
  const pending = (page) => editRow(page, edited).getByRole("button", { name: /^Undoing: Updated task/ });
  await expect(pending(reader)).toHaveText("Undoing");
  await expect(pending(reader)).toBeDisabled();
  const undoRecord = '"op":"undo"';
  await committed(reader, undoRecord);
  await expect(pending(observer)).toBeDisabled();
  await pending(observer).dispatchEvent("click"); // aria-disabled alone does not block a click.
  await reader.reload();
  await expect(pending(reader)).toBeDisabled();
  expect(await stored(reader, undoRecord)).toBe(1);
  await context.unroute(undoPath);
  await author.evaluate(() => window.dispatchEvent(new Event("online")));
  await expect(edit.getByText("Undone", { exact: true })).toBeVisible({ timeout: 15_000 });
  await expect(row(author, original)).toBeVisible();
  await expect(row(author, edited)).toHaveCount(0);
  await reader.getByRole("button", { name: "Refresh activity" }).click();
  const compensation = reader.locator(".activity-row").first();
  await expect(compensation).toContainText(`Undo: Updated task "${original}" (title)`);
  await compensation.getByRole("button", { name: /^Undo:/ }).click();
  await expect(row(author, edited)).toBeVisible();
  const label = (row) => row.getByText("Undo unavailable", { exact: true });
  const loadedBeforeRedo = editRow(reader, original);
  await expect(label(loadedBeforeRedo)).toBeVisible();
  await expect(loadedBeforeRedo.getByRole("button")).toHaveCount(0);
  await reader.reload();
  const loadedAfterRedo = editRow(reader, edited);
  await expect(label(loadedAfterRedo)).toBeVisible();
  await expect(loadedAfterRedo.getByRole("button")).toHaveCount(0);

  expect(problems.flat()).toEqual([]);
  await observer.close();
  await reader.close();
  await author.close();
});

test("Activity queues a refresh during pagination and focuses its heading when a row is removed", async () => {
  const author = await openApp();
  const reader = await openApp();
  const problems = [author, reader].map(watch);
  const older = Promise.withResolvers();
  const releaseOlder = Promise.withResolvers();
  const refresh = Promise.withResolvers();
  const releaseRefresh = Promise.withResolvers();
  let latestReads = 0;
  await reader.route("**/api/v1/activity?*", async (route) => {
    const url = new URL(route.request().url());
    // Real server pages, but make the first page short enough for one edit to replace its row.
    url.searchParams.set("limit", "1");
    if (url.searchParams.has("before")) {
      older.resolve();
      await releaseOlder.promise;
    } else if (++latestReads === 2) {
      refresh.resolve();
      await releaseRefresh.promise;
    }
    await route.fulfill({ response: await route.fetch({ url: url.href }) });
  });
  try {
    await reader.getByRole("link", { name: "Activity", exact: true }).click();
    const first = reader.locator(".activity-row").first();
    const undo = first.getByRole("button", { name: /^Undo:/ });
    await expect(undo).toBeEnabled();
    const previousSummary = await first.locator(".activity-summary").textContent();

    await reader.getByRole("button", { name: "Load older activity" }).click();
    await older.promise;
    await reader.getByRole("button", { name: "Refresh activity" }).click();
    expect(latestReads).toBe(1);
    const title = "Newest change after paging started";
    await add(author, title);
    await expect.poll(() => occurrences(author, title)).toBe(1);

    releaseOlder.resolve();
    await refresh.promise;
    await undo.focus();
    await expect(undo).toBeFocused();
    releaseRefresh.resolve();
    await expect(first).toContainText(title);
    await expect(reader.getByText(previousSummary, { exact: true })).toHaveCount(0);
    await expect(reader.getByRole("heading", { name: "Activity", exact: true })).toBeFocused();
    expect(problems.flat()).toEqual([]);
  } finally {
    releaseOlder.resolve();
    releaseRefresh.resolve();
    await reader.close();
    await author.close();
  }
});

test("attachments wait for task creation and event delivery; the web session can then download and remove them", async () => {
  const page = await context.newPage();
  const problems = watch(page);
  const title = "Draft a letter";
  const name = "Příloha + notes & plan.txt";
  const buffer = Buffer.from("Example attachment\n");
  // Pause delivery inside the page without changing what the real server sends or stores.
  await page.addInitScript(() => {
    const fetch = window.fetch.bind(window);
    let gate;
    let release;
    window.holdReplicaEvents = () => { gate = new Promise((resolve) => { release = resolve; }); };
    window.releaseReplicaEvents = () => { gate = undefined; release?.(); };
    window.fetch = async (request, init) => {
      const response = await fetch(request, init);
      if (!(request instanceof Request) || new URL(request.url).pathname !== "/api/v1/events" || !response.ok) {
        return response;
      }
      const stream = response.body.pipeThrough(new TransformStream({
        async transform(chunk, controller) { await gate; controller.enqueue(chunk); },
      }));
      return new Response(stream, { status: response.status, headers: response.headers });
    };
  });
  await page.goto("/app");
  await expect(page.locator(".sync-text")).toHaveAttribute("title", /^Synced/);
  let allowCreate, allowUpload;
  const createGate = new Promise((resolve) => { allowCreate = resolve; });
  const uploadGate = new Promise((resolve) => { allowUpload = resolve; });
  const uploadPath = /\/api\/v1\/tasks\/[^/]+\/attachments\?/;
  let uploads = 0;
  await page.route("**/api/v1/commands", async (route) => {
    if (route.request().postDataJSON()?.command?.title === title) await createGate;
    await route.continue();
  });
  await page.route(uploadPath, async (route) => {
    uploads++;
    await uploadGate;
    await route.continue();
  });
  const section = page.getByRole("region", { name: "Attachments" });
  const addFile = section.getByRole("button", { name: "Add file" });
  const status = section.getByRole("status");
  const file = section.getByRole("button", { name: `Download “${name}”` });
  let response;
  try {
    await add(page, title);
    await row(page, title).click();
    const picker = page.waitForEvent("filechooser");
    await addFile.click();
    await (await picker).setFiles({ name, mimeType: "text/plain", buffer });
    await expect(status).toHaveText("Waiting to sync…");
    await expect(addFile).toBeDisabled();
    expect(await occurrences(page, title)).toBe(0);
    expect(uploads).toBe(0);

    allowCreate();
    await expect.poll(() => uploads).toBe(1);
    await expect(status).toHaveText("Uploading…");
    await page.evaluate(() => window.holdReplicaEvents());
    const upload = page.waitForResponse((r) => r.request().method() === "POST" && uploadPath.test(r.url()));
    allowUpload();
    response = await upload;
    expect(response.status()).toBe(201);
    await expect(status).toHaveText("Waiting to sync…");
    await expect(file).toHaveCount(0);
    await expect(addFile).toBeDisabled();
  } finally {
    allowCreate();
    allowUpload();
    await page.evaluate(() => window.releaseReplicaEvents());
  }
  const { attachment } = await response.json();
  expect(attachment).toMatchObject({ file_name: name, media_type: "text/plain", size_bytes: buffer.length });

  await expect(file).toBeVisible(); // Arrived through the event stream, not from a local placeholder.
  await expect(addFile).toBeEnabled();
  expect(uploads).toBe(1);
  const downloading = page.waitForEvent("download");
  await file.click();
  const download = await downloading;
  expect(download.suggestedFilename()).toBe(name);
  expect(readFileSync(await download.path())).toEqual(buffer);

  await section.getByRole("button", { name: `Remove “${name}”` }).click();
  await expect(file).toHaveCount(0);
  await expect(page.getByRole("button", { name: /^Undo/ })).toBeVisible();
  await expect.poll(() => page.evaluate(async (id) => {
    const csrf = document.querySelector('meta[name="csrf-token"]').content;
    return (await fetch(`/api/v1/attachments/${id}`, { method: "HEAD", headers: { "X-CSRF-Token": csrf } })).status;
  }, attachment.id)).toBe(404);
  expect(problems).toEqual([]);
  await page.close();
});

test("upload refusals show fixed wording, not the server message", async () => {
  const page = await openApp();
  const problems = watch(page);
  await row(page, "Draft a letter").click();
  const section = page.getByRole("region", { name: "Attachments" });
  const uploadPath = /\/api\/v1\/tasks\/[^/]+\/attachments\?/;
  for (const [status, max, message] of [
    [413, 25 * 1024 * 1024, "That file is too large. The limit is 25 MiB."],
    [413, "untrusted data", "That file is too large."],
    [507, null, "Attachment storage is full."],
    [429, null, "Couldn’t upload right now. Try again later."],
  ]) {
    await page.route(uploadPath, (route) => route.fulfill({
      status,
      json: { error: { code: "refused", message: "untrusted data", details: { max_bytes: max } } },
    }), { times: 1 });
    await section.locator('input[type="file"]').setInputFiles({
      name: "notes.txt", mimeType: "text/plain", buffer: Buffer.from("Notes"),
    });
    await expect(section.getByRole("alert")).toHaveText(message);
  }
  expect(problems).toEqual([]);
  await page.close();
});

test("set a repeat in detail, then complete it and see the next occurrence from the server", async () => {
  const page = await openApp();
  const problems = watch(page);
  const list = "0193b2c0-0000-7000-8000-000000000001";
  const title = "Refill the seed trays";
  await page.locator(".sidebar").getByRole("link", { name: /^Inbox/ }).click();
  await add(page, title);
  await row(page, title).locator(".row-title").click();
  await page.getByRole("button", { name: "Repeat", exact: true }).click();
  await page.getByRole("dialog", { name: "Repeat", exact: true }).getByRole("option", { name: "Daily" }).click();
  await expect(page.getByRole("button", { name: "Repeat", exact: true })).toHaveText("Every day");
  const items = () => page.evaluate(async ({ list, title }) => {
    const csrf = document.querySelector('meta[name="csrf-token"]').content;
    const res = await fetch(`/api/v1/lists/${list}/tasks`, { headers: { "X-CSRF-Token": csrf } });
    if (!res.ok) throw new Error(`Read list: ${res.status}`);
    return (await res.json()).items.filter((t) => t.title === title);
  }, { list, title });
  await expect.poll(items).toEqual([expect.objectContaining({
    due: expect.stringMatching(/^\d{4}-\d{2}-\d{2}$/),
    recurrence: expect.objectContaining({ rule: { freq: "daily", interval: 1 } }),
  })]);
  const [first] = await items();
  await row(page, title).getByRole("checkbox").click();
  await expect.poll(items).toHaveLength(2);
  const next = (await items()).find((t) => t.id !== first.id);
  expect(next.due > first.due).toBe(true);
  expect(next.completed_at).toBeNull();
  await expect(row(page, title)).toHaveCount(1); // the original finishes its completion motion first
  await expect(row(page, title).locator(".row-title")).toHaveAttribute("id", `row-${next.id}`);
  expect(problems).toEqual([]);
  await page.close();
});

test("another tab undoes both the repeat and its first due date on a previously undated task", async () => {
  const author = await openApp();
  const reader = await openApp();
  const problems = [author, reader].map(watch);
  const title = "Order labels for the seed trays";
  const repeat = (tab) => tab.getByRole("button", { name: "Repeat", exact: true });
  const due = (tab) => tab.getByRole("button", { name: /^Edit due date:/ });
  await add(author, title);
  for (const tab of [author, reader]) {
    await row(tab, title).locator(".row-title").click();
    await expect(due(tab)).toHaveText("Set date");
  }
  await repeat(author).click();
  await author.getByRole("dialog", { name: "Repeat" }).getByRole("option", { name: "Weekdays" }).click();
  await expect(repeat(reader)).toHaveText("Every weekday");
  await expect(due(reader)).not.toHaveText("Set date");
  await reader.getByRole("link", { name: "Activity", exact: true }).click();
  const edit = reader.locator(".activity-row").filter({ hasText: `Updated task "${title}"` });
  await expect(edit).toContainText("due date");
  await expect(edit).toContainText("recurrence");
  await edit.getByRole("button", { name: /^Undo:/ }).click();
  await expect(edit.getByText("Undone", { exact: true })).toBeVisible();
  await expect(repeat(author)).toHaveText("Does not repeat");
  await expect(due(author)).toHaveText("Set date");
  expect(problems.flat()).toEqual([]);
  await reader.close();
  await author.close();
});

test("reminder edits and undo sync across tabs, and a stale picker cannot overwrite another edit", async () => {
  const a = await openApp();
  const b = await openApp();
  const problems = [a, b].map(watch);
  const title = "Check the greenhouse vents";
  const staleText = "The reminder changed while the picker was open. Check it, then try again.";
  const reminder = (tab) => tab.getByRole("button", { name: "Reminder", exact: true });
  const presets = (tab) => tab.getByRole("dialog", { name: "Reminder", exact: true });
  const custom = async (tab) => {
    await reminder(tab).click();
    await presets(tab).getByRole("option", { name: "Custom…" }).click();
    return tab.getByRole("dialog", { name: "Custom reminder", exact: true });
  };
  for (const tab of [a, b]) {
    const cdp = await context.newCDPSession(tab);
    await cdp.send("Emulation.setTimezoneOverride", { timezoneId: "Europe/Prague" });
  }
  await add(a, title);
  for (const tab of [a, b]) await row(tab, title).locator(".row-title").click();
  const task = (await row(a, title).locator(".row-title").getAttribute("id")).replace(/^row-/, "");
  const reminders = () => b.evaluate(async (task) => {
    const csrf = document.querySelector('meta[name="csrf-token"]').content;
    const res = await fetch(`/api/v1/tasks/${task}`, { headers: { "X-CSRF-Token": csrf } });
    if (!res.ok) throw new Error(`Read task: ${res.status}`);
    return (await res.json()).reminders;
  }, task);

  const empty = await custom(b);
  await reminder(a).click();
  await presets(a).getByRole("option", { name: "Tomorrow at 09:00" }).click();
  await expect(reminder(b)).toHaveText("Tomorrow, 09:00");
  await empty.getByLabel("Time", { exact: true }).fill("10:30");
  await empty.getByRole("button", { name: "Save reminder" }).click();
  await expect(empty.getByRole("alert")).toHaveText(staleText);
  await b.keyboard.press("Escape");
  await expect(reminder(b)).toBeFocused();
  await expect.poll(reminders).toHaveLength(1);
  const [original] = await reminders();
  const stale = await custom(a);
  const editor = await custom(b);
  const save = editor.getByRole("button", { name: "Save reminder" });
  await editor.getByLabel("Date", { exact: true }).fill("2032-03-28");
  await editor.getByLabel("Time", { exact: true }).fill("02:30");
  await expect(editor.getByRole("status")).toContainText("That time doesn’t exist");
  await expect(save).toBeDisabled();
  await editor.getByLabel("Time", { exact: true }).fill("03:30");
  await save.click();
  await expect(reminder(a)).toContainText("03:30");
  const updated = expect.objectContaining({ id: original.id, at: "2032-03-28T01:30:00Z" });
  await expect.poll(reminders).toEqual([updated]);

  await stale.getByLabel("Time", { exact: true }).fill("10:30");
  await stale.getByRole("button", { name: "Save reminder" }).click();
  await expect(stale.getByRole("alert")).toHaveText(staleText);
  await expect(stale.getByLabel("Time", { exact: true })).toHaveValue("10:30");
  await expect(reminder(a)).toContainText("03:30");
  await a.keyboard.press("Escape");
  await expect(reminder(a)).toBeFocused();

  await reminder(b).click();
  await presets(b).getByRole("button", { name: "Remove reminder" }).click();
  await expect(reminder(a)).toHaveText("Set reminder");
  await expect.poll(reminders).toEqual([]);
  await reminder(b).press("Control+z");
  await expect(reminder(a)).toContainText("03:30");
  await expect.poll(reminders).toEqual([updated]);
  expect(problems.flat()).toEqual([]);
  await b.close();
  await a.close();
});

test("a repeated local time starts at the earlier instant, but an unchanged later instant stays put", async () => {
  const page = await openApp();
  const problems = watch(page);
  const cdp = await context.newCDPSession(page);
  await cdp.send("Emulation.setTimezoneOverride", { timezoneId: "Europe/Prague" });
  const title = "Check the greenhouse thermostat";
  await add(page, title);
  const taskRow = row(page, title).locator(".row-title");
  await taskRow.click();
  const task = (await taskRow.getAttribute("id")).replace(/^row-/, "");
  const reminder = page.getByRole("button", { name: "Reminder", exact: true });
  const editor = page.getByRole("dialog", { name: "Custom reminder", exact: true });
  const date = editor.getByLabel("Date", { exact: true });
  const time = editor.getByLabel("Time", { exact: true });
  const save = editor.getByRole("button", { name: "Save reminder" });
  const openCustom = async () => {
    await reminder.click();
    await page.getByRole("option", { name: "Custom…", exact: true }).click();
    await expect(date).toBeFocused();
  };
  const saved = () => page.evaluate(async (task) => {
    const csrf = document.querySelector('meta[name="csrf-token"]').content;
    const res = await fetch(`/api/v1/tasks/${task}`, { headers: { "X-CSRF-Token": csrf } });
    if (!res.ok) throw new Error(`Read task: ${res.status}`);
    return (await res.json()).reminders;
  }, task);
  await openCustom();
  await date.fill("2032-10-31");
  await time.fill("02:30");
  await save.click();
  await expect(reminder).toContainText("02:30");
  await page.locator(".toast").getByRole("button", { name: "Dismiss" }).click();
  await expect.poll(saved).toEqual([expect.objectContaining({ at: "2032-10-31T00:30:00Z" })]);
  const [{ id }] = await saved();

  // Observe an intermediate wall time over SSE before choosing the second 02:30 with an explicit
  // offset. That way we know the picker is not still seeing the first 02:30 it originally set.
  const move = async (at) => {
    const status = await page.evaluate(async ({ id, at }) => {
      const csrf = document.querySelector('meta[name="csrf-token"]').content;
      return (await fetch("/api/v1/commands", {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-CSRF-Token": csrf },
        body: JSON.stringify({ command_id: crypto.randomUUID(), command: { cmd: "update_reminder", id, at } }),
      })).status;
    }, { id, at });
    expect(status).toBe(200);
  };
  await move("2032-10-31T02:00:00Z");
  await expect(reminder).toContainText("03:00");
  const later = "2032-10-31T01:30:00Z";
  await move(later);
  await expect(reminder).toContainText("02:30");
  await openCustom();
  await expect(date).toHaveValue("2032-10-31");
  await expect(time).toHaveValue("02:30");
  await save.click();
  await expect(reminder).toBeFocused();
  await expect(page.locator(".toast").getByRole("button", { name: /^Undo/ })).toHaveCount(0);
  await expect.poll(saved).toEqual([expect.objectContaining({ id, at: later })]);
  expect(problems).toEqual([]);
  await page.close();
});

test("tags and links sync across tabs, and delete rechecks the tag’s impact at confirmation", async () => {
  const page = await openApp();
  const other = await openApp();
  const firstProblems = watch(page);
  const otherProblems = watch(other);
  const title = "Pick swatches for the guide";
  const second = "Review the guide layout";
  const tag = "guide-review";
  const href = "https://example.com/guide";
  const select = (tab, name) => row(tab, name).locator(".row-title").click();
  const tags = (tab) => tab.getByRole("button", { name: "Tags", exact: true });
  const picker = (tab) => tab.getByRole("dialog", { name: "Tags", exact: true });
  const link = (tab) => tab.locator(`#task-detail a[href="${href}"]`);
  await add(page, title);
  await add(page, second);
  await select(page, title);
  await tags(page).click();
  const filter = picker(page).getByRole("textbox", { name: "Type to find or create a tag" });
  await filter.fill(tag);
  await filter.press("Enter");
  await expect(tags(page)).toContainText(tag);
  await filter.press("Escape");
  const input = page.getByRole("textbox", { name: "Add a web link" });
  await input.fill("example.com/guide");
  await input.press("Enter");

  await select(other, title);
  await expect(tags(other)).toContainText(tag);
  await expect(link(other)).toBeVisible();
  await page.getByRole("button", { name: /Remove link/ }).click();
  await expect(link(other)).toHaveCount(0);
  await page.locator(".room-title").focus();
  await page.keyboard.press("Control+z");
  await expect(link(other)).toBeVisible();
  await page.getByRole("button", { name: /Remove link/ }).click();
  await expect(link(other)).toHaveCount(0);

  await tags(page).click();
  await picker(page).getByRole("button", { name: tag, exact: true }).click();
  await expect(tags(other)).not.toContainText(tag);
  await picker(page).getByRole("button", { name: tag, exact: true }).click();
  await expect(tags(other)).toContainText(tag);
  await picker(page).getByRole("button", { name: `Manage tag “${tag}”` }).click();
  await page.getByRole("button", { name: "Delete tag…" }).click();
  const confirm = page.getByRole("dialog", { name: "Delete tag" });
  await expect(confirm).toContainText("It’s on 1 task.");

  // The owner added the tag to another task since the question was presented.
  await select(other, second);
  await tags(other).click();
  await picker(other).getByRole("button", { name: tag, exact: true }).click();
  await expect(row(page, second)).toContainText(`#${tag}`);
  const remove = confirm.getByRole("button", { name: "Delete tag", exact: true });
  await remove.click();
  await expect(confirm).toContainText("It’s on 2 tasks.");
  await expect(confirm.getByRole("alert")).toHaveText("The tag changed while the question was open. Check it, then confirm again.");
  await expect(confirm.getByRole("button", { name: "Cancel" })).toBeFocused();
  await expect(tags(other)).toContainText(tag);
  await remove.click();
  await expect(tags(other)).not.toContainText(tag);
  await page.locator(".room-title").focus();
  await page.keyboard.press("Control+z");
  await expect(tags(other)).toContainText(tag);
  expect([...firstProblems, ...otherProblems]).toEqual([]);
  await other.close();
  await page.close();
});

test("undo of a tag delete reports a name clash; losing the task closes the tag confirmation", async () => {
  const author = await openApp();
  const other = await openApp();
  const problems = [author, other].map(watch);
  const original = "Review the ink samples";
  const second = "Review the paper samples";
  const tag = "print-proof";
  const tags = (tab) => tab.getByRole("button", { name: "Tags", exact: true });
  const picker = (tab) => tab.getByRole("dialog", { name: "Tags", exact: true });
  const confirm = author.getByRole("dialog", { name: "Delete tag", exact: true });
  const create = async (tab) => {
    await tags(tab).click();
    const filter = picker(tab).getByRole("textbox", { name: "Type to find or create a tag" });
    await filter.fill(tag);
    await filter.press("Enter");
    await expect(tags(tab)).toHaveAccessibleDescription(tag);
    await filter.press("Escape");
  };
  const askDelete = async () => {
    await tags(author).click();
    await picker(author).getByRole("button", { name: `Manage tag “${tag}”` }).click();
    await author.getByRole("button", { name: "Delete tag…" }).click();
    await expect(confirm).toContainText(`Delete the tag “${tag}”?`);
  };
  await add(author, original);
  await add(author, second);
  const originalRow = row(author, original).locator(".row-title");
  const id = (await originalRow.getAttribute("id")).replace(/^row-/, "");
  await originalRow.click();
  await create(author);
  await expect(row(other, original)).toContainText(`#${tag}`);
  await askDelete();
  await confirm.getByRole("button", { name: "Delete tag", exact: true }).click();
  await expect(row(other, original)).not.toContainText(`#${tag}`);
  await row(other, second).locator(".row-title").click();
  await create(other);
  await expect(row(author, second)).toContainText(`#${tag}`);

  const refusal = author.waitForResponse((r) => /\/api\/v1\/events\/\d+\/undo$/.test(r.url()));
  await author.locator(".room-title").focus();
  await author.keyboard.press("Control+z");
  const response = await refusal;
  expect(response.status()).toBe(409);
  expect((await response.json()).error.code).toBe("duplicate_name");
  await expect(author.locator(".toast")).toContainText("Couldn't undo: another tag already uses that name.");
  await expect(tags(author)).toHaveAccessibleDescription("No tags");
  await expect(tags(other)).toHaveAccessibleDescription(tag);

  await askDelete();
  const status = await other.evaluate(async (id) => {
    const csrf = document.querySelector('meta[name="csrf-token"]').content;
    return (await fetch("/api/v1/commands", {
      method: "POST",
      headers: { "X-CSRF-Token": csrf, "content-type": "application/json" },
      body: JSON.stringify({ command_id: crypto.randomUUID(), command: { cmd: "delete", id: { kind: "task", id } } }),
    })).status;
  }, id);
  expect(status).toBe(200);
  await expect(confirm).toBeHidden();
  await expect(originalRow).toHaveCount(0);
  await expect(tags(other)).toHaveAccessibleDescription(tag);
  await expect(other.locator(".sync-text")).toContainText("1 not synced");
  await author.getByRole("button", { name: "1 couldn't sync" }).click();
  const failures = author.getByRole("dialog", { name: "Not synced" });
  await failures.getByRole("button", { name: "Dismiss", exact: true }).click();
  await expect(author.locator(".sync-text")).not.toContainText("not synced");
  await expect.poll(() => failedCount(author)).toBe(0);
  expect(problems.flat()).toEqual([]);
  await other.close();
  await author.close();
});

test("move a list between web-created collections and see the new parent from another tab", async () => {
  const page = await openApp();
  const other = await openApp();
  const problems = watch(page);
  const collection = (tab, name) => tab.locator(".nav-section").filter({
    has: tab.getByRole("link", { name: `${name} 0`, exact: true }),
  });
  for (const name of ["Greenhouse", "Workshop"]) {
    await page.getByRole("button", { name: "New", exact: true }).click();
    await page.getByRole("menuitem", { name: "Collection", exact: true }).click();
    const input = page.getByRole("textbox", { name: "Collection name" });
    await input.fill(name);
    await input.press("Enter");
    await expect(page.locator(".room-title")).toHaveText(name);
  }
  await collection(page, "Greenhouse").getByRole("link", { name: "Greenhouse 0", exact: true }).click();
  await page.getByRole("button", { name: "New", exact: true }).click();
  await page.getByRole("menuitem", { name: "List", exact: true }).click();
  const list = page.getByRole("textbox", { name: "List name" });
  await list.fill("Propagation");
  await list.press("Enter");
  const link = (tab, name) => collection(tab, name).getByRole("link", { name: "Propagation 0", exact: true });
  await expect(link(other, "Greenhouse")).toBeVisible();
  await page.getByRole("button", { name: "Room menu" }).click();
  await page.getByRole("menuitem", { name: "Move to…" }).click();
  await page.getByRole("dialog", { name: "Move to" }).getByRole("option", { name: "Workshop" }).click();
  await expect(link(other, "Greenhouse")).toHaveCount(0);
  await expect(link(other, "Workshop")).toBeVisible();
  await page.locator(".room-title").focus();
  await page.keyboard.press("Control+z");
  await expect(link(other, "Greenhouse")).toBeVisible();
  await expect(link(other, "Workshop")).toHaveCount(0);
  expect(problems).toEqual([]);
  await other.close();
  await page.close();
});

test("tracked undo of a confirmed recurring completion restores only the original", async () => {
  const page = await openApp();
  const problems = watch(page);
  const list = "0193b2c0-0000-7000-8000-000000000001";
  const id = "0193b2c0-0000-7000-8000-0000000000b1";
  const title = "Water the balcony planters";
  const due = "2027-06-01";
  const created = await page.evaluate(async ({ list, id, title, due }) => {
    const csrf = document.querySelector('meta[name="csrf-token"]').content;
    const res = await fetch("/api/v1/commands", {
      method: "POST",
      headers: { "X-CSRF-Token": csrf, "content-type": "application/json" },
      body: JSON.stringify({
        command_id: "0193b2c0-0000-7000-8000-0000000000b2",
        command: { cmd: "create_task", id, list, title, due, recurrence: { rule: { freq: "daily", interval: 1 }, until: null } },
      }),
    });
    return { status: res.status, body: await res.text() };
  }, { list, id, title, due });
  expect(created.status, created.body).toBe(200);
  const occurrencesInList = () => page.evaluate(async ({ list, title }) => {
    const csrf = document.querySelector('meta[name="csrf-token"]').content;
    const res = await fetch(`/api/v1/lists/${list}/tasks`, { headers: { "X-CSRF-Token": csrf } });
    if (!res.ok) throw new Error(`Read list: ${res.status}`);
    return (await res.json()).items.filter((t) => t.title === title);
  }, { list, title });

  await page.locator(".sidebar").getByRole("link", { name: /^Inbox/ }).click();
  await expect(row(page, title).locator(".row-title")).toHaveAttribute("id", `row-${id}`);
  await row(page, title).getByRole("checkbox").click();
  await expect.poll(occurrencesInList).toHaveLength(2);
  const completed = await occurrencesInList();
  expect(completed.find((t) => t.id === id).completed_at).toBeTruthy();
  expect(completed.find((t) => t.id !== id).due).not.toBe(due);
  await expect(page.locator(".sync-text")).toHaveAttribute("title", /^Synced/);

  const compensation = page.waitForResponse((r) => /\/api\/v1\/events\/\d+\/undo$/.test(r.url()));
  // Focus outside the input: Ctrl+Z still works even if the toast has already elapsed.
  await page.locator(".room-title").focus();
  await page.keyboard.press("Control+z");
  expect((await compensation).ok()).toBe(true);
  await expect.poll(occurrencesInList).toEqual([expect.objectContaining({ id, due, completed_at: null })]);
  await expect(row(page, title)).toHaveCount(1);
  await expect(row(page, title).locator(".row-title")).toHaveAttribute("id", `row-${id}`);
  expect(problems).toEqual([]);
  await page.close();
});

test("a follower undoes a recurring completion the sender has adopted; its late receipt keeps the newer Undo", async () => {
  const sender = await openApp();
  const follower = await openApp();
  const problems = watch(follower);
  const title = "Water the balcony planters";
  const nextTitle = "Check the watering schedule";
  const original = "0193b2c0-0000-7000-8000-0000000000b1";
  const sync = follower.locator(".sync-text");
  await follower.locator(".sidebar").getByRole("link", { name: /^Inbox/ }).click();
  await add(follower, nextTitle);
  await expect(sync).toHaveAttribute("title", /^Synced/);

  let releaseComplete, releaseUndo, sawComplete, sawUndo;
  const completeGate = new Promise((resolve) => { releaseComplete = resolve; });
  const undoGate = new Promise((resolve) => { releaseUndo = resolve; });
  const sendingComplete = new Promise((resolve) => { sawComplete = resolve; });
  const sendingUndo = new Promise((resolve) => { sawUndo = resolve; });
  await sender.route("**/api/v1/commands", async (route) => {
    if (route.request().postDataJSON().command.id === original) {
      sawComplete();
      await completeGate;
    }
    await route.continue();
  });
  await sender.route(/\/api\/v1\/events\/\d+\/undo$/, async (route) => {
    sawUndo();
    await undoGate;
    await route.continue();
  });
  try {
    await row(follower, title).getByRole("checkbox").click();
    await sendingComplete; // The sender has its own copy; the follower's attempt count is still 0.
    await follower.getByRole("button", { name: /^Undo/ }).click();
    await committed(follower, '"op":"undo_after"');
    await committed(follower, '"cmd":"complete_task"');
    releaseComplete();
    await sendingUndo;

    // Leave the first undo in flight while creating a newer undoable toast in the follower.
    await row(follower, nextTitle).getByRole("checkbox").click();
    await expect(follower.locator(".toast")).toContainText(nextTitle);
    releaseUndo();
    await expect(sync).toHaveAttribute("title", /^Synced/);
    await expect(row(follower, title)).toHaveCount(1);
    await expect(row(follower, title).locator(".row-title")).toHaveAttribute("id", `row-${original}`);
    await expect(row(follower, title).getByRole("checkbox")).not.toBeChecked();
    await expect(follower.locator(".toast")).toContainText(nextTitle);
    await follower.getByRole("button", { name: /^Undo/ }).click();
    await expect(sync).toHaveAttribute("title", /^Synced/);
    await expect(row(follower, nextTitle).getByRole("checkbox")).not.toBeChecked();
    await expect(follower.locator(".toast")).toContainText(`Undid: Completed “${nextTitle}”`);
    expect(problems).toEqual([]);
  } finally {
    releaseComplete();
    releaseUndo();
    await sender.close();
    await follower.close();
  }
});

test("a live sidebar update cannot retarget a highlighted room action", async () => {
  const page = await openApp();
  const problems = watch(page);
  const id = (n) => `0193b2c0-0000-7000-8000-0000000000c${n}`;
  const send = async (command_id, command) => {
    const res = await page.evaluate(async ({ command_id, command }) => {
      const csrf = document.querySelector('meta[name="csrf-token"]').content;
      const res = await fetch("/api/v1/commands", {
        method: "POST",
        headers: { "X-CSRF-Token": csrf, "content-type": "application/json" },
        body: JSON.stringify({ command_id, command }),
      });
      return { status: res.status, body: await res.text() };
    }, { command_id, command });
    expect(res.status, res.body).toBe(200);
  };
  await send(id(1), { cmd: "create_list", id: id(2), name: "Menu target" });
  const sidebar = page.locator(".sidebar");
  const target = sidebar.getByRole("link", { name: /^Menu target/ });
  await target.press("Shift+F10");
  await expect(page.getByRole("menuitem", { name: "Rename list" })).toBeFocused();
  await page.keyboard.press("ArrowDown");
  const up = page.getByRole("menuitem", { name: "Move up" });
  await expect(up).toBeFocused();

  await send(id(3), { cmd: "create_list", id: id(4), name: "New sibling", placement: { at: "before", sibling: { kind: "list", id: id(2) } } });
  const order = sidebar.locator('.nav-item:not(.sub)[href^="/app/list/"] .nav-name');
  await expect(order).toHaveText(["Inbox", "New sibling", "Menu target"]);
  await expect(up).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(order).toHaveText(["Inbox", "Menu target", "New sibling"]);
  await expect(page.locator(".sync-text")).toHaveAttribute("title", /^Synced/);
  await page.keyboard.press("Control+z");
  await expect(order).toHaveText(["Inbox", "New sibling", "Menu target"]);
  await expect(page.locator(".sync-text")).toHaveAttribute("title", /^Synced/);
  for (const [cmd, list] of [[5, 2], [6, 4]]) {
    await send(id(cmd), { cmd: "delete", id: { kind: "list", id: id(list) } });
  }
  await expect(order).toHaveText(["Inbox"]);
  expect(problems).toEqual([]);
  await page.close();
});

test("Dismiss all does not drop a refusal that arrived after opening Not synced", async () => {
  const page = await openApp();
  let release;
  const waitForDialog = new Promise((resolve) => { release = resolve; });
  await page.route("**/api/v1/commands", async (route) => {
    if (route.request().postDataJSON().command.title === "Late refusal") await waitForDialog;
    await route.fulfill({ status: 409, json: { error: { code: "deleted", message: "Raw response details" } } });
  });
  const input = page.getByRole("textbox", { name: "Add a task" });
  for (const title of ["First refusal", "Late refusal"]) {
    await input.fill(title);
    await input.press("Enter");
  }
  const dialog = page.getByRole("dialog", { name: "Not synced" });
  try {
    await page.getByRole("button", { name: "1 couldn't sync" }).click();
    await expect(dialog.getByRole("listitem")).toHaveCount(1);
    await expect(dialog).toContainText("First refusal");
    await expect(dialog).not.toContainText("Raw response details");
  } finally {
    release();
  }
  await expect(page.locator(".sync-text")).toContainText("2 not synced");
  await dialog.getByRole("button", { name: "Dismiss all" }).click();
  await expect(dialog).toContainText("No more changes to dismiss.");
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await page.getByRole("button", { name: "1 couldn't sync" }).click();
  await expect(dialog.getByRole("listitem")).toHaveCount(1);
  await expect(dialog).toContainText("Late refusal");
  await dialog.getByRole("button", { name: "Dismiss", exact: true }).click();
  await expect(page.locator(".sync-text")).not.toContainText("not synced");
  // Let the deletion persist before closing this tab.
  await expect.poll(() => failedCount(page)).toBe(0);
  await page.close();
});

test("two tabs queue edits offline: both survive closing every tab, and each is sent once", async () => {
  const a = await openApp();
  const b = await openApp();
  const problems = [...watch(a), ...watch(b)];
  await context.route("**/api/v1/commands", (r) => r.abort("internetdisconnected"));
  // Each tab has its own copy of the replica over the same IndexedDB database.
  await add(a, "Z karty A offline");
  await add(b, "Z karty B offline");
  await add(a, "Zase z karty A");
  await expect(a.locator(".sync-text")).not.toHaveAttribute("title", /^Synced/, { timeout: 15_000 });
  await expect(b.locator(".sync-text")).not.toHaveAttribute("title", /^Synced/, { timeout: 15_000 });
  for (const t of ["Z karty A offline", "Z karty B offline", "Zase z karty A"]) await committed(a, t);
  expect(problems).toEqual([]);
  await a.close();
  await b.close();
  await context.unroute("**/api/v1/commands");

  // Reopen: every tab's edit is still queued, and is sent.
  const c = await openApp();
  for (const t of ["Z karty A offline", "Z karty B offline", "Zase z karty A"]) {
    await expect(row(c, t)).toHaveCount(1);
    await expect.poll(() => occurrences(c, t), { timeout: 15_000 }).toBe(1);
  }
  await expect(c.locator(".sync-text")).toHaveAttribute("title", /^Synced/, { timeout: 15_000 });
  // And only once, even after another reload.
  await c.reload();
  await expect(c.locator(".sync-text")).toHaveAttribute("title", /^Synced/, { timeout: 15_000 });
  for (const t of ["Z karty A offline", "Z karty B offline", "Zase z karty A"]) {
    expect(await occurrences(c, t)).toBe(1);
    await expect(row(c, t)).toHaveCount(1);
  }
  await c.close();
});

test("a tab whose session changed under it still sends its queued edit, once", async ({ browser }) => {
  const a = await openApp();
  const problems = watch(a);
  await a.route("**/api/v1/commands", (r) => r.abort("internetdisconnected"));
  await add(a, "Po novém přihlášení");
  await committed(a, "Po novém přihlášení");

  // Sign in again elsewhere and swap that session's cookie in, as a sign-in from another tab does:
  // the cookie now holds a new session with a new CSRF token, and the first tab's token is stale.
  const elsewhere = await browser.newContext({ baseURL });
  const signIn = await elsewhere.newPage();
  await authenticator(signIn, elsewhere);
  await signIn.goto("/login?next=/admin");
  await signIn.getByRole("button", { name: "Sign in with passkey" }).click();
  await signIn.waitForURL((u) => u.pathname.startsWith("/admin"));
  const fresh = (await elsewhere.cookies()).find((c) => c.name === "__Host-todo_session");
  expect(fresh).toBeTruthy();
  await elsewhere.close();
  await context.addCookies([fresh]);
  expect((await context.cookies()).filter((c) => c.name === fresh.name).map((c) => c.value)).toEqual([fresh.value]);
  const b = await context.newPage();
  const oldToken = await a.evaluate(() => document.querySelector('meta[name="csrf-token"]').content);
  const refused = await a.evaluate(async (csrf) => {
    const res = await fetch("/api/v1/lists", { headers: { "X-CSRF-Token": csrf } });
    return res.status;
  }, oldToken);
  expect(refused).toBe(403);

  // The first tab reaches the server again: it gets 403 csrf, fetches the new token, and resends
  // the same command.
  await a.unroute("**/api/v1/commands");
  await a.evaluate(() => window.dispatchEvent(new Event("online")));
  await b.goto("/app");
  await expect.poll(() => occurrences(b, "Po novém přihlášení"), { timeout: 20_000 }).toBe(1);
  await expect(a.locator(".sync-text")).toHaveAttribute("title", /^Synced/, { timeout: 15_000 });
  await expect(a.locator(".sync-text")).not.toContainText("not synced");
  // A new edit from the old tab goes through too, on the new token.
  await add(a, "A ještě jedna");
  await expect.poll(() => occurrences(b, "A ještě jedna"), { timeout: 15_000 }).toBe(1);
  expect(await occurrences(b, "Po novém přihlášení")).toBe(1);
  expect(problems).toEqual([]);
  await a.close();
  await b.close();
});

test("an Activity-only 401 asks for the passkey again without persisting sign-out", async ({ browser }) => {
  const isolated = await browser.newContext({ baseURL });
  try {
    const cookies = await context.cookies();
    await isolated.addCookies(cookies);
    const page = await isolated.newPage();
    const signIn = page.getByRole("button", { name: "Sign in with passkey" });
    await page.goto("/app");
    await expect(page.locator(".sync-text")).toHaveAttribute("title", /^Synced/, { timeout: 15_000 });

    // Count attempts, including any navigation that gets cancelled before it commits.
    const cdp = await isolated.newCDPSession(page);
    await cdp.send("Page.enable");
    const redirects = [];
    cdp.on("Page.frameRequestedNavigation", ({ url }) => {
      if (new URL(url).pathname === "/login") redirects.push(url);
    });

    // A reconnect can fail offline, but it can't receive the 401 before Activity does.
    await page.route("**/api/v1/events**", (r) => r.abort("internetdisconnected"));
    await isolated.clearCookies();
    const [refusal] = await Promise.all([
      page.waitForResponse((r) => new URL(r.url()).pathname === "/api/v1/activity"),
      page.getByRole("link", { name: "Activity", exact: true }).click(),
    ]);
    expect(refusal.status()).toBe(401);
    await expect(signIn).toBeVisible();
    expect(new URL(page.url()).searchParams.get("next")).toBe("/app/activity");
    expect(redirects).toEqual([page.url()]);

    // Restore the valid session cookie. The fresh app page must not inherit the old latch.
    await page.unroute("**/api/v1/events**");
    await isolated.addCookies(cookies);
    await page.goto("/app/activity");
    await expect(page).toHaveURL(/\/app\/activity$/);
    await expect(page.locator(".sync-text")).toHaveAttribute("title", /^Synced/, { timeout: 15_000 });
  } finally {
    await isolated.close();
  }
});

test("a revoked tab goes to sign-in only after storing its pending edit", async ({ browser }) => {
  const cookies = await context.cookies();
  const isolated = await browser.newContext({ baseURL });
  try {
    await isolated.addCookies(cookies);
    const page = await isolated.newPage();
    await page.addInitScript(injectStorageFailure);
    await page.goto("/app");
    await expect(page.locator(".sync-text")).toHaveAttribute("title", /^Synced/, { timeout: 15_000 });
    await page.evaluate(() => { window.__failIdb = true; });
    const title = "Queued before sign-out";
    await add(page, title);
    await expect(page.locator(".toast-error[role=alert]")).toContainText("Couldn’t save in this browser");

    await page.route("**/api/v1/events**", (r) => r.abort("internetdisconnected"));
    const dialogs = [];
    page.on("dialog", async (d) => {
      dialogs.push(d.type());
      await d.dismiss();
    });
    await isolated.clearCookies();
    await page.getByRole("link", { name: "Activity", exact: true }).click();
    await expect(page.locator(".sync-text")).toHaveText("Signed out");
    await expect(page).toHaveURL(/\/app\/activity$/);

    await page.evaluate(() => { window.__failIdb = false; });
    await expect(page.getByRole("button", { name: "Sign in with passkey" })).toBeVisible();
    expect(dialogs).toEqual([]);
    await page.unroute("**/api/v1/events**");
    await isolated.addCookies(cookies);
    await page.goto("/app");
    await expect.poll(() => occurrences(page, title), { timeout: 15_000 }).toBe(1);
  } finally {
    await isolated.close();
  }
});

test("a note being typed survives a rename from elsewhere, and then saves", async () => {
  const a = await openApp();
  const problems = watch(a);
  await add(a, "Koncept zprávy");
  await row(a, "Koncept zprávy").click();
  await expect(a.locator(".d-title")).toHaveText("Koncept zprávy");
  const id = /\/task\/([0-9a-f-]+)/.exec(a.url())?.[1];
  expect(id).toBeTruthy();
  await expect.poll(() => occurrences(a, "Koncept zprávy"), { timeout: 15_000 }).toBe(1);

  // Typing a note: not saved yet (it saves when the field loses focus).
  const notes = a.locator("textarea.notes");
  await notes.click();
  await notes.pressSequentially("Rozepsaná poznámka");
  await expect(notes).toBeFocused();

  // Meanwhile the task is renamed elsewhere, and the live update reaches this tab.
  const b = await openApp();
  const renamed = await b.evaluate(async (id) => {
    const csrf = document.querySelector('meta[name="csrf-token"]').content;
    const res = await fetch("/api/v1/commands", {
      method: "POST",
      headers: { "X-CSRF-Token": csrf, "content-type": "application/json", accept: "application/json" },
      body: JSON.stringify({
        command_id: crypto.randomUUID(),
        command: { cmd: "update_task", id, patch: { title: "Koncept zprávy pro tým" } },
      }),
    });
    return { status: res.status, body: await res.text() };
  }, id);
  expect(renamed.status, renamed.body).toBeLessThan(300);
  await expect(a.locator(".d-title")).toHaveText("Koncept zprávy pro tým", { timeout: 15_000 });

  // The draft is still there, still focused, and keeps going.
  await expect(notes).toHaveValue("Rozepsaná poznámka");
  await expect(notes).toBeFocused();
  await notes.pressSequentially(" a dopsaná");
  await notes.blur();
  await expect
    .poll(async () => (await serverLog(b)).body, { timeout: 15_000 })
    .toContain("Rozepsaná poznámka a dopsaná");
  // The rename stands.
  await expect(row(b, "Koncept zprávy pro tým")).toHaveCount(1, { timeout: 15_000 });
  expect(problems).toEqual([]);
  await a.close();
  await b.close();
});

test("one tab sends for all: a stalled tab's rename and a later one from another tab land in order", async () => {
  // `a` opens first, so it holds the sender lock; `b` only queues, and tells `a`.
  const a = await openApp();
  const b = await openApp();
  const problems = [...watch(a), ...watch(b)];
  await add(a, "Pořadí");
  await expect.poll(() => occurrences(a, "Pořadí"), { timeout: 15_000 }).toBe(1);
  await expect(row(b, "Pořadí")).toHaveCount(1, { timeout: 15_000 });

  // `a` stalls with its rename in flight.
  await a.route("**/api/v1/commands", (r) => r.abort("internetdisconnected"));
  await rename(a, "Pořadí", "Pořadí z karty A");
  await committed(a, "Pořadí z karty A");
  // Then `b` renames the same task (it hasn't seen `a`'s pending rename).
  await rename(b, "Pořadí", "Pořadí z karty B");
  await committed(b, "Pořadí z karty B");
  // `a` takes `b`'s edit in and shows it; nothing has reached the server yet.
  await expect(a.locator(".d-title")).toHaveText("Pořadí z karty B", { timeout: 15_000 });
  expect(await occurrences(a, "Pořadí z karty A")).toBe(0);
  expect(await occurrences(a, "Pořadí z karty B")).toBe(0);

  // `a` gets through: its rename first, then `b`'s, so `b`'s title wins.
  await a.unroute("**/api/v1/commands");
  await a.evaluate(() => window.dispatchEvent(new Event("online")));
  await expect.poll(() => occurrences(b, "Pořadí z karty B"), { timeout: 20_000 }).toBe(1);
  const { body } = await serverLog(b);
  expect(body.indexOf("Pořadí z karty A")).toBeGreaterThan(-1);
  expect(body.indexOf("Pořadí z karty A")).toBeLessThan(body.indexOf("Pořadí z karty B"));
  for (const page of [a, b]) {
    await expect(page.locator(".d-title")).toHaveText("Pořadí z karty B", { timeout: 15_000 });
    await expect(page.locator(".sync-text")).toHaveAttribute("title", /^Synced/, { timeout: 15_000 });
  }
  // `a`'s title is in the log twice: as its rename's new title, and as the old title `b`'s rename
  // replaced (the event records it), which only happens in this order.
  expect(await occurrences(b, "Pořadí z karty A")).toBe(2);
  expect(problems).toEqual([]);
  await a.close();

  // The lock passes to `b`, which now sends its own edits.
  await add(b, "Po předání zámku");
  await expect.poll(() => occurrences(b, "Po předání zámku"), { timeout: 20_000 }).toBe(1);
  await b.close();
});

test("a new sender reads what the old one left before it sends its own: the newer rename wins", async () => {
  // Reproduced by review: `a` sends for both, stalls with an older rename, and closes; `b` takes over
  // holding a newer rename of the same task, and must not send it before the older one.
  const a = await openApp();
  const b = await openApp();
  const problems = [...watch(a), ...watch(b)];
  await add(a, "Předání původní");
  await expect(row(b, "Předání původní")).toHaveCount(1, { timeout: 15_000 });
  await a.route("**/api/v1/commands", (r) => r.abort("internetdisconnected"));
  await rename(a, "Předání původní", "Předání starší");
  await committed(a, "Předání starší");
  await rename(b, "Předání původní", "Předání novější");
  await committed(b, "Předání novější");
  await a.close();
  await expect
    .poll(
      async () => {
        const { body } = await serverLog(b);
        return body.includes("Předání starší") && body.includes("Předání novější");
      },
      { timeout: 15_000 },
    )
    .toBe(true);
  const { body } = await serverLog(b);
  expect(body.indexOf("Předání starší"), "the older rename lands first").toBeLessThan(body.indexOf("Předání novější"));
  await expect(b.locator(".d-title")).toHaveText("Předání novější", { timeout: 15_000 });
  await expect(b.locator(".sync-text")).toHaveAttribute("title", /^Synced/, { timeout: 15_000 });
  expect(problems).toEqual([]);
  await b.close();
});

test("the sender finds an edit whose announcement was missed, and a failing read says so", async () => {
  await context.addInitScript(() => {
    const post = BroadcastChannel.prototype.postMessage;
    BroadcastChannel.prototype.postMessage = function (...args) {
      if (window.__muteBroadcast) return undefined;
      return post.apply(this, args);
    };
    const getAll = IDBObjectStore.prototype.getAll;
    IDBObjectStore.prototype.getAll = function (...args) {
      if (window.__failScan) throw new DOMException("injected read failure", "UnknownError");
      return getAll.apply(this, args);
    };
  });
  const a = await openApp();
  const b = await openApp();
  const problems = [...watch(a), ...watch(b)];
  await add(a, "Zmeškané oznámení");
  await expect(row(b, "Zmeškané oznámení")).toHaveCount(1, { timeout: 15_000 });
  // `b`'s announcement is lost: `a` still finds the edit by reading the outbox store.
  await b.evaluate(() => {
    window.__muteBroadcast = true;
  });
  await rename(b, "Zmeškané oznámení", "Zmeškané oznámení nalezeno");
  await committed(b, "Zmeškané oznámení nalezeno");
  await expect.poll(() => occurrences(a, "Zmeškané oznámení nalezeno"), { timeout: 12_000 }).toBe(1);

  // Reading the store keeps failing in the sender: the alert shows until it works again.
  await a.evaluate(() => {
    window.__failScan = true;
  });
  const alert = a.locator(".toast-error[role=alert]");
  await expect(alert).toHaveAttribute("title", /injected read failure/, { timeout: 15_000 });
  await a.evaluate(() => {
    window.__failScan = false;
  });
  await expect(alert).toHaveCount(0, { timeout: 10_000 });
  expect(problems).toEqual([]);
  await a.close();
  await b.close();
});

test("an edit this browser can't store is never sent, says so, and goes through once storage works", async () => {
  // Every page of the context can be made to fail IndexedDB writes (as when the quota runs out).
  await context.addInitScript(injectStorageFailure);
  const a = await openApp();
  const problems = watch(a);
  await a.evaluate(() => {
    window.__failIdb = true;
  });
  await add(a, "Nedá se uložit");
  const alert = a.locator(".toast-error[role=alert]");
  await expect(alert).toContainText("Couldn’t save in this browser. Retrying; keep this tab open.", {
    timeout: 10_000,
  });
  await expect(alert).toHaveAttribute("title", /injected write failure/);
  // Not stored, so not sent: the server never hears of an edit a closed tab would forget.
  await a.waitForTimeout(1500);
  expect(await stored(a, "Nedá se uložit")).toBe(0);
  expect(await occurrences(a, "Nedá se uložit")).toBe(0);
  await expect(alert).toBeVisible();

  // Closing the tab now asks first.
  let asked = "";
  a.once("dialog", async (d) => {
    asked = d.type();
    await d.dismiss();
  });
  await a.close({ runBeforeUnload: true });
  await expect.poll(() => asked, { timeout: 5_000 }).toBe("beforeunload");
  expect(a.isClosed()).toBe(false);

  // Storage works again: the retry stores it, the alert goes, and it is sent exactly once (and
  // then leaves the outbox, so it is not waited for there).
  await a.evaluate(() => {
    window.__failIdb = false;
  });
  await expect(alert).toHaveCount(0, { timeout: 10_000 });
  await expect.poll(() => occurrences(a, "Nedá se uložit"), { timeout: 15_000 }).toBe(1);
  await expect(a.locator(".sync-text")).toHaveAttribute("title", /^Synced/, { timeout: 15_000 });
  expect(problems).toEqual([]);
  await a.close();
});

test("a version 2 sender retires before a new tab can queue guarded moves", async ({ browser }) => {
  const mixed = await browser.newContext({ baseURL, storageState: await context.storageState() });
  try {
    const old = await mixed.newPage();
    // Version 2 has the same stores and upgrade protocol. Pin just this tab's open to 2 so it
    // exercises the real retire handler without building and hosting a second bundle in CI.
    await old.addInitScript(() => {
      const open = IDBFactory.prototype.open;
      IDBFactory.prototype.open = function (name, version) {
        return name === "todo-replica" ? open.call(this, name, 2) : open.call(this, name, version);
      };
    });
    await old.goto("/app");
    await expect(old.locator(".sync-text")).toHaveAttribute("title", /^Synced/);
    await expect.poll(() => old.evaluate(async () =>
      (await navigator.locks.query()).held.some((lock) => lock.name === "todo-sender")
    )).toBe(true);
    const title = "Task from the older tab";
    await add(old, title);
    await expect.poll(() => occurrences(old, title)).toBe(1);

    // Leave the old page on screen so opening the new app must retire it, not rely on navigation
    // releasing its sender lock. It should be read-only even with the reload suppressed.
    await old.route("**/*", (r) => r.request().isNavigationRequest()
      ? r.fulfill({ status: 204 }) : r.continue());
    const current = await mixed.newPage();
    await current.goto("/app");
    await expect(old.locator(".toast-read-only")).toBeVisible();
    await expect(old.locator(".qa-input")).toBeDisabled();
    await expect(current.locator(".sync-text")).toHaveAttribute("title", /^Synced/);

    const destination = "Guarded move destination";
    await createRoom(current, "list", destination);
    await listLink(current, "Inbox").click();
    const from = new URL(current.url()).pathname.split("/").at(-1);
    await taskButton(current, title).press("m");
    const sent = current.waitForResponse((response) => {
      const request = response.request();
      return request.method() === "POST" && request.url().endsWith("/api/v1/commands") &&
        request.postDataJSON()?.command?.cmd === "move_task";
    });
    await movePicker(current).getByRole("option", { name: destination }).click();
    const response = await sent;
    expect(response.ok()).toBe(true);
    expect(response.request().postDataJSON().command.from_list).toBe(from);
    await listLink(current, destination).click();
    await expect(taskButton(current, title)).toBeVisible();
    await deleteRoom(current, "list");
  } finally {
    await mixed.close();
  }
});

test("a tab still reading its replica retires when a newer tab opens", async ({ browser }) => {
  const mixed = await browser.newContext({ baseURL, storageState: await context.storageState() });
  try {
    const old = await mixed.newPage();
    await old.addInitScript(() => {
      const open = IDBFactory.prototype.open;
      IDBFactory.prototype.open = function (name, version) {
        if (name !== "todo-replica") return open.call(this, name, version);
        const request = open.call(this, name, 2);
        request.addEventListener("success", () => {
          request.result.addEventListener("versionchange", () => { window.__upgradeRequested = true; });
        });
        return request;
      };
      // Hold delivery of the last replica read. The actual transaction can finish, so the
      // connection (not an artificially open transaction) is what can block the upgrade.
      const getAll = IDBObjectStore.prototype.getAll;
      const success = Object.getOwnPropertyDescriptor(IDBRequest.prototype, "onsuccess");
      IDBObjectStore.prototype.getAll = function (...args) {
        const request = getAll.apply(this, args);
        if (this.name === "damaged") {
          Object.defineProperty(request, "onsuccess", {
            set(handler) {
              success.set.call(this, (event) => {
                window.__finishReplicaRead = () => handler.call(this, event);
              });
            },
          });
        }
        return request;
      };
    });
    await old.goto("/app");
    await old.waitForFunction(() => typeof window.__finishReplicaRead === "function");
    // Keep the old document here when it tries to reload so its startup outcome is observable.
    await old.route("**/app", (r) => r.fulfill({ status: 204 }));
    const current = await mixed.newPage();
    await current.goto("/app");
    await old.waitForFunction(() => window.__upgradeRequested);
    // The newer tab must finish opening even while the old tab's startup is still paused.
    await expect(current.locator(".sync-text")).toHaveAttribute("title", /^Synced/, { timeout: 10_000 });
    const reload = old.waitForRequest((r) => r.isNavigationRequest());
    await old.evaluate(() => window.__finishReplicaRead());
    await reload;
    await expect(old.locator(".fatal")).toContainText("another tab is updating the local database");
    await expect(old.locator(".qa-input")).toHaveCount(0);
  } finally {
    await mixed.close();
  }
});

// The last tests replace the database (everything above is synced, so nothing unsent is lost).

test("an upgrade blocked by another tab says so, and goes ahead once that tab lets go", async () => {
  const old = await context.newPage();
  await old.goto("/health");
  // A connection at version 1 that doesn't close when asked (an old tab).
  await old.evaluate(
    () =>
      new Promise((resolve, reject) => {
        const del = indexedDB.deleteDatabase("todo-replica");
        del.onerror = () => reject(del.error);
        del.onsuccess = () => {
          const open = indexedDB.open("todo-replica", 1);
          open.onupgradeneeded = () => open.result.createObjectStore("kv");
          open.onerror = () => reject(open.error);
          open.onsuccess = () => {
            window.__db = open.result;
            resolve();
          };
        };
      }),
  );
  const app = await context.newPage();
  const problems = watch(app);
  await app.goto("/app");
  await expect(app.locator("#boot-notice")).toHaveText("Close other To-do tabs to finish updating", {
    timeout: 10_000,
  });
  await old.evaluate(() => window.__db.close());
  await expect(app.locator("#boot-notice")).toHaveCount(0, { timeout: 10_000 });
  await expect(app.locator(".room-title")).toHaveText("My Day");
  await expect(app.locator(".sync-text")).toHaveAttribute("title", /^Synced/, { timeout: 15_000 });
  await expect(row(app, "Po předání zámku")).toHaveCount(1, { timeout: 15_000 });
  expect(problems).toEqual([]);
  await app.close();
  await old.close();
});

test("a tab with changes it couldn't save keeps an upgrade waiting until they're stored", async () => {
  await context.addInitScript(injectStorageFailure);
  const app = await openApp();
  const problems = watch(app);
  await app.evaluate(() => {
    window.__failIdb = true;
  });
  await add(app, "Čeká na uložení");
  await expect(app.locator(".toast-error[role=alert]")).toBeVisible({ timeout: 10_000 });

  // A newer version asks for the database: this tab can't let go yet.
  const newer = await context.newPage();
  await newer.goto("/health");
  await newer.evaluate(() => {
    window.__upgrade = "waiting";
    const open = indexedDB.open("todo-replica", 4);
    open.onblocked = () => {
      window.__blocked = true;
    };
    open.onsuccess = () => {
      open.result.close();
      window.__upgrade = "opened";
    };
    open.onerror = () => {
      window.__upgrade = String(open.error);
    };
  });
  await expect(app.locator(".toast-holding[role=alert]")).toContainText(
    "This tab has changes it couldn’t save yet; other tabs will update once they’re saved.",
    { timeout: 10_000 },
  );
  await expect.poll(() => newer.evaluate(() => window.__blocked === true), { timeout: 10_000 }).toBe(true);
  await app.waitForTimeout(1500);
  expect(await newer.evaluate(() => window.__upgrade)).toBe("waiting");

  // (The reloaded page meets the newer database and stops; only this build's errors count.)
  expect([...problems]).toEqual([]);

  // Storage works again: the change is stored, then the tab lets go and reloads.
  const reloaded = app.waitForEvent("load", { timeout: 15_000 });
  await app.evaluate(() => {
    window.__failIdb = false;
  });
  await reloaded;
  await expect.poll(() => newer.evaluate(() => window.__upgrade), { timeout: 10_000 }).toBe("opened");
  // Nothing was lost: the change is in the database the newer version took over.
  expect(await stored(newer, "Čeká na uložení")).toBe(1);

  // Clean up: back to this build's database. (The change goes with it; it was never sent, because
  // this tab stopped syncing when the newer version asked.)
  await newer.evaluate(
    () =>
      new Promise((resolve, reject) => {
        const del = indexedDB.deleteDatabase("todo-replica");
        del.onsuccess = () => resolve();
        del.onerror = () => reject(del.error);
      }),
  );
  expect(await occurrences(app, "Čeká na uložení")).toBe(0);
  await app.reload();
  await expect(app.locator(".sync-text")).toHaveAttribute("title", /^Synced/, { timeout: 15_000 });
  await app.close();
  await newer.close();
});

test("once a newer version takes the database over, the tab takes no edits and says so", async () => {
  const app = await openApp();
  const problems = watch(app);
  await add(app, "Jen ke čtení");
  await expect.poll(() => occurrences(app, "Jen ke čtení"), { timeout: 15_000 }).toBe(1);
  await row(app, "Jen ke čtení").click();
  await expect(app.locator(".d-title")).toHaveText("Jen ke čtení");
  // The reload the tab tries after closing its connection answers 204, so the page stays (found by
  // review: the tab then went on taking edits that no database would ever hold).
  let reloads = 0;
  await app.route("**/*", (r) => {
    if (!r.request().isNavigationRequest()) return r.continue();
    reloads += 1;
    return r.fulfill({ status: 204 });
  });
  const newer = await context.newPage();
  await newer.goto("/health");
  const opened = await newer.evaluate(
    () =>
      new Promise((resolve, reject) => {
        const open = indexedDB.open("todo-replica", 4);
        open.onerror = () => reject(open.error);
        open.onsuccess = () => {
          open.result.close();
          resolve("opened");
        };
      }),
  );
  expect(opened).toBe("opened");
  await expect.poll(() => reloads, { timeout: 10_000 }).toBeGreaterThan(0);

  const alert = app.locator(".toast-read-only[role=alert]");
  await expect(alert).toContainText("A newer version is open in another tab. Reload to keep editing");
  await expect(alert.getByRole("button", { name: "Reload" })).toBeVisible();
  await expect(app.locator(".qa-input")).toBeDisabled();
  await expect(row(app, "Jen ke čtení").locator(".check")).toBeDisabled();
  await expect(app.locator(".check.big")).toBeDisabled();

  // The keyboard can't edit either: Space doesn't complete, E doesn't open the title editor.
  await app.keyboard.press(" ");
  await app.keyboard.press("e");
  // And an edit forced past the disabled field is refused by the backend, applying nothing.
  await app.evaluate(() => {
    const input = document.querySelector(".qa-input");
    input.disabled = false;
    input.value = "Nemá se objevit";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.form.requestSubmit();
  });
  await app.waitForTimeout(500);
  await expect(row(app, "Jen ke čtení")).toHaveCount(1);
  await expect(row(app, "Jen ke čtení").locator(".check")).toHaveAttribute("aria-checked", "false");
  await expect(app.locator(".title-input")).toHaveCount(0);
  await expect(row(app, "Nemá se objevit")).toHaveCount(0);
  await expect(app.locator(".toast:not(.toast-error)")).toHaveCount(0);
  await expect(app.locator(".toast-announcement")).toBeEmpty();
  await expect(alert).toBeVisible();
  expect(await stored(newer, "Nemá se objevit")).toBe(0);
  expect(await occurrences(app, "Nemá se objevit")).toBe(0);
  expect(problems).toEqual([]);

  // Clean up: back to this build's database.
  await app.unroute("**/*");
  await newer.evaluate(
    () =>
      new Promise((resolve, reject) => {
        const del = indexedDB.deleteDatabase("todo-replica");
        del.onsuccess = () => resolve();
        del.onerror = () => reject(del.error);
      }),
  );
  await app.reload();
  await expect(app.locator(".sync-text")).toHaveAttribute("title", /^Synced/, { timeout: 15_000 });
  await expect(row(app, "Jen ke čtení")).toHaveCount(1, { timeout: 15_000 });
  await app.close();
  await newer.close();
});

test("a tab lets go of the database when a newer version needs it, and reloads", async () => {
  const app = await openApp();
  const newer = await context.newPage();
  await newer.goto("/health");
  const reloaded = app.waitForEvent("load", { timeout: 15_000 });
  // A newer build opens version 4: the app writes what it has, closes its connection (so the
  // upgrade goes ahead), and reloads to get the newer build.
  const opened = await newer.evaluate(
    () =>
      new Promise((resolve, reject) => {
        const open = indexedDB.open("todo-replica", 4);
        open.onerror = () => reject(open.error);
        open.onsuccess = () => {
          open.result.close();
          resolve("opened");
        };
      }),
  );
  expect(opened).toBe("opened");
  await reloaded;
  // Clean up: back to this build's database (refetched from the server).
  await newer.evaluate(
    () =>
      new Promise((resolve, reject) => {
        const del = indexedDB.deleteDatabase("todo-replica");
        del.onsuccess = () => resolve();
        del.onerror = () => reject(del.error);
      }),
  );
  await app.reload();
  await expect(app.locator(".sync-text")).toHaveAttribute("title", /^Synced/, { timeout: 15_000 });
  await app.close();
  await newer.close();
});

// Run last with a separate replica so the fake clock and its write cannot affect the shared-tab tests.
test("quick-add uses the local date at submit and My Day refreshes on wake", async ({ browser }) => {
  const isolated = await browser.newContext({ baseURL, timezoneId: "UTC", storageState: await context.storageState() });
  try {
    const page = await isolated.newPage();
    await page.clock.setFixedTime(new Date("2026-11-20T23:59:59Z"));
    await page.goto("/app");
    await expect(page.locator(".sync-text")).toHaveAttribute("title", /^Synced/, { timeout: 15_000 });

    // Change Date without running the view's 30-second interval or waking the tab.
    const title = "Check midnight quick-add";
    await page.locator(".qa-input").fill(`${title} today`);
    await page.clock.setFixedTime(new Date("2026-11-21T00:00:00Z"));
    const sent = page.waitForRequest((r) =>
      r.method() === "POST" && r.url().endsWith("/api/v1/commands") &&
      r.postDataJSON()?.command?.title === title
    );
    await page.locator(".qa-input").press("Enter");
    const { command } = (await sent).postDataJSON();
    expect(command).toMatchObject({ due: "2026-11-21", my_day: "2026-11-21" });
    await expect(row(page, title)).toHaveCount(1);

    await page.clock.setFixedTime(new Date("2026-11-22T00:00:00Z"));
    await page.evaluate(() => window.dispatchEvent(new Event("online")));
    await expect(row(page, title)).toHaveCount(0);
  } finally {
    await isolated.close();
  }
});
