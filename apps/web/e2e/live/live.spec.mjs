// The real thing: sign in with a passkey (a virtual authenticator), then use the app against the server.
// Covers the session + CSRF path of the API, the event stream, and an edit that survives a reload while
// the server can't be reached.
import { expect, test } from "@playwright/test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { DATA } from "../live-env.mjs";

test.describe.configure({ mode: "serial" });

const rows = (page) => page.locator(".rows:not(.completed-rows) .row");
const row = (page, title) => rows(page).filter({ hasText: title });

/** The server's event log as the app's session sees it. */
async function serverLog(page) {
  return page.evaluate(async () => {
    const csrf = document.querySelector('meta[name="csrf-token"]').content;
    const res = await fetch("/api/v1/events?since=0&limit=500", {
      headers: { "X-CSRF-Token": csrf, accept: "application/json" },
    });
    return { status: res.status, body: await res.text() };
  });
}

test("set up, sign in, and use the app against the server", async ({ page, context }) => {
  const problems = [];
  page.on("pageerror", (e) => problems.push(e.message));
  page.on("console", (m) => {
    if (/Content.Security.Policy/i.test(m.text())) problems.push(m.text());
  });

  const cdp = await context.newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  await cdp.send("WebAuthn.addVirtualAuthenticator", {
    options: {
      protocol: "ctap2",
      transport: "internal",
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
      automaticPresenceSimulation: true,
    },
  });

  // First run: the setup code from the data directory, then a passkey.
  await page.goto("/setup");
  // The file's first line is the code; the rest explains it.
  const code = readFileSync(join(DATA, "setup-code.txt"), "utf8").trim().split(/\s+/)[0];
  await page.locator('input[name="code"]').fill(code);
  await page.getByRole("button", { name: "Register passkey" }).click();
  await page.waitForURL((u) => !u.pathname.startsWith("/setup"));

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
  await page.locator(".qa-input").fill("Zalít květiny");
  await page.locator(".qa-input").press("Enter");
  await expect(row(page, "Zalít květiny")).toHaveCount(1);
  await expect.poll(async () => (await serverLog(page)).body, { timeout: 15_000 }).toContain("Zalít květiny");

  // The server can't be reached: the edit waits in the outbox (IndexedDB).
  await page.route("**/api/v1/commands", (r) => r.abort("internetdisconnected"));
  await page.locator(".qa-input").fill("Koupit mléko offline");
  await page.locator(".qa-input").press("Enter");
  await expect(row(page, "Koupit mléko offline")).toHaveCount(1);
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
  await other.locator(".qa-input").fill("Z druhé záložky");
  await other.locator(".qa-input").press("Enter");
  await expect(row(page, "Z druhé záložky")).toHaveCount(1, { timeout: 15_000 });
  await other.close();

  // Completing works against the server too.
  await row(page, "Zalít květiny").locator(".check").click();
  await expect(row(page, "Zalít květiny")).toHaveCount(0, { timeout: 5_000 });
  await expect.poll(async () => (await serverLog(page)).body, { timeout: 15_000 }).toContain('"field":"completed_at"');

  // A request without the CSRF header is refused.
  const bare = await page.evaluate(async () => (await fetch("/api/v1/events?since=0")).status);
  expect(bare).toBe(403);

  expect(problems).toEqual([]);
});
