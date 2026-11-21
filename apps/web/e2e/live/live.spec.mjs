// The real thing: sign in with a passkey (a virtual authenticator), then use the app against the server.
// Covers the session + CSRF path of the API, the event stream, and edits that survive a reload while
// the server can't be reached, from one tab or several.
//
// The tests share one browser context (one cookie jar, like the owner's browser) and run in order:
// the first one sets the server up.
import { expect, test } from "@playwright/test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { DATA, PORT } from "../live-env.mjs";

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
async function authenticator(page, ctx = context) {
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
  for (const credential of credentials) {
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
  await expect(page.locator(".sync-text")).toHaveText(/^Synced/, { timeout: 15_000 });
  return page;
}

async function add(page, title) {
  await page.locator(".qa-input").fill(title);
  await page.locator(".qa-input").press("Enter");
  await expect(row(page, title)).toHaveCount(1);
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
  await expect(page.locator(".sync-text")).toHaveText(/^Synced/, { timeout: 15_000 });

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
  await expect(page.locator(".sync-text")).not.toHaveText(/^Synced/, { timeout: 15_000 });
  expect((await serverLog(page)).body).not.toContain("Koupit mléko offline");
  await page.waitForTimeout(500);

  // Reload with the server reachable again: the pending edit comes back from IndexedDB and is sent.
  await page.unroute("**/api/v1/commands");
  await page.reload();
  await expect(row(page, "Koupit mléko offline")).toHaveCount(1);
  await expect.poll(async () => (await serverLog(page)).body, { timeout: 15_000 }).toContain("Koupit mléko offline");
  await expect(page.locator(".sync-text")).toHaveText(/^Synced/, { timeout: 15_000 });

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

  expect(problems).toEqual([]);
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
  await expect(a.locator(".sync-text")).not.toHaveText(/^Synced/, { timeout: 15_000 });
  await expect(b.locator(".sync-text")).not.toHaveText(/^Synced/, { timeout: 15_000 });
  await a.waitForTimeout(500);
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
  await expect(c.locator(".sync-text")).toHaveText(/^Synced/, { timeout: 15_000 });
  // And only once, even after another reload.
  await c.reload();
  await expect(c.locator(".sync-text")).toHaveText(/^Synced/, { timeout: 15_000 });
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
  await a.waitForTimeout(500);

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
  await expect(a.locator(".sync-text")).toHaveText(/^Synced/, { timeout: 15_000 });
  await expect(a.locator(".sync-text")).not.toContainText("not synced");
  // A new edit from the old tab goes through too, on the new token.
  await add(a, "A ještě jedna");
  await expect.poll(() => occurrences(b, "A ještě jedna"), { timeout: 15_000 }).toBe(1);
  expect(await occurrences(b, "Po novém přihlášení")).toBe(1);
  expect(problems).toEqual([]);
  await a.close();
  await b.close();
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
