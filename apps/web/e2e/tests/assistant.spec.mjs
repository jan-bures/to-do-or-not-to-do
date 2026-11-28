import { createServer } from "node:http";
import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";
import { MODEL_ORIGIN, aiSettings, call, chunk, configure, modelRoute, panel, prompt, send, toolResults, words } from "./assistant-helpers.mjs";

const trigger = (page) => page.getByRole("button", { name: "Ask AI", exact: true });

test("Ask AI keyboard toggle, modal settings, and persistent keyboard/pointer splitter", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await open(page);
  await trigger(page).focus();
  await page.keyboard.press("Control+j");
  await expect(trigger(page)).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByRole("button", { name: "This device", exact: true })).toBeFocused();
  await page.getByRole("button", { name: "This device", exact: true }).click();
  const settings = aiSettings(page);
  await expect(settings.getByLabel("Base URL")).toBeFocused();
  await expect(settings).toHaveCSS("width", "520px");
  await settings.getByRole("button", { name: "Save", exact: true }).press("Tab");
  await expect(settings.getByRole("button", { name: "Close AI settings" })).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(settings.getByRole("button", { name: "Save", exact: true })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(settings).not.toBeVisible();
  await expect(page.getByRole("button", { name: "This device", exact: true })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(trigger(page)).toBeFocused();
  await expect(panel(page)).not.toBeVisible();
  await page.keyboard.press("Meta+j");
  await expect(page.getByRole("button", { name: "This device", exact: true })).toBeFocused();
  const divider = page.getByRole("separator", { name: "Resize AI assistant" });
  await divider.press("Shift+ArrowLeft");
  await expect(divider).toHaveAttribute("aria-valuenow", "432");
  const box = await divider.boundingBox();
  await page.mouse.move(box.x + 5, box.y + 120);
  await page.mouse.down();
  await page.mouse.move(box.x - 35, box.y + 120);
  await page.mouse.up();
  await expect(divider).toHaveAttribute("aria-valuenow", "472");
  await page.reload();
  await trigger(page).click();
  await expect(panel(page)).toHaveCSS("width", "472px");
  await divider.press("Home");
  await expect(panel(page)).toHaveCSS("width", "400px");
  await page.getByRole("button", { name: "Close assistant" }).click();
  await expect(trigger(page)).toBeFocused();
  await w.check();
});

test("medium drawer and narrow sheet contain focus and preserve the draft", async ({ page }) => {
  const w = watch(page);
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.setViewportSize({ width: 1024, height: 768 });
  await open(page);
  await trigger(page).click();
  const drawer = page.getByRole("dialog", { name: "AI assistant" });
  await expect(drawer).toHaveCSS("width", "400px");
  await expect(drawer).toHaveCSS("transform", "none");
  await configure(page);
  await expect(prompt(page)).toHaveValue("");
  await page.getByRole("button", { name: "What’s due this week?" }).click();
  await expect(prompt(page)).toBeFocused();
  await expect(prompt(page)).toHaveValue("What’s due this week?");
  await prompt(page).press("Shift+Enter");
  await expect(prompt(page)).toHaveValue("What’s due this week?\n");
  await page.mouse.click(520, 380);
  await expect(drawer).not.toBeVisible();
  await expect(trigger(page)).toBeFocused();
  await page.setViewportSize({ width: 390, height: 844 });
  await trigger(page).click();
  await expect(drawer).toBeVisible();
  const box = await drawer.boundingBox();
  expect(box.height).toBeCloseTo(844 * 0.88, 0);
  expect(box.y + box.height).toBeCloseTo(844, 0);
  await expect(page.locator("main")).toHaveJSProperty("inert", true);
  await expect(page.getByRole("separator", { name: "Resize AI assistant" })).toHaveCount(0);
  await expect(prompt(page)).toHaveValue("What’s due this week?\n");
  await expect(prompt(page)).toBeFocused();
  await page.getByRole("button", { name: "Send", exact: true }).press("Tab");
  await expect(drawer.getByRole("button", { name: "AI settings" })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(prompt(page)).toHaveValue("");
  await page.keyboard.press("Escape");
  await expect(drawer).not.toBeVisible();
  await expect(trigger(page)).toBeFocused();
  await w.check();
});

test("task links keep both panes visible and only dock detail from 1600 px", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await open(page);
  await trigger(page).click();
  await configure(page, { Planning: "all" });
  await modelRoute(page, (body) => toolResults(body).length === 0
    ? call("find", "query_tasks", { query: "November board update" })
    : words("I found Prepare the November board update."));
  await send(page, "Find the board update");
  const task = "Prepare the November board update";
  const link = panel(page).getByRole("button", { name: new RegExp(task) });
  await expect(link).toBeVisible();
  await expect(panel(page).getByText(/Read 1 task ·/)).toBeVisible();
  await link.click();
  const detail = page.locator("#task-detail");
  await expect(detail).toHaveCSS("position", "fixed");
  await expect(panel(page)).toBeVisible();
  await expect(detail.locator(".d-title")).toHaveText(task);
  await page.setViewportSize({ width: 1600, height: 900 });
  await expect(detail).not.toHaveCSS("position", "fixed");
  await expect(page.getByRole("separator")).toHaveCount(2);
  const detailPane = await detail.boundingBox();
  const assistant = await panel(page).boundingBox();
  expect(detailPane.x + detailPane.width).toBeCloseTo(assistant.x - 1, 0);
  await page.setViewportSize({ width: 1440, height: 900 });
  await expect(detail).toHaveCSS("position", "fixed");
  await page.keyboard.press("Escape");
  await expect(detail).not.toBeVisible();
  await expect(panel(page)).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(panel(page)).not.toBeVisible();
  await w.check();
});

test("streams an unfinished reply and Stop aborts its fetch", async ({ page }) => {
  const w = watch(page);
  let observed;
  let disconnected = false;
  const server = createServer(async (req, res) => {
    res.setHeader("Access-Control-Allow-Origin", "*");
    res.setHeader("Access-Control-Allow-Headers", "content-type");
    res.setHeader("Access-Control-Allow-Methods", "POST, OPTIONS");
    if (req.method === "OPTIONS") { res.writeHead(204).end(); return; }
    let body = "";
    for await (const chunk of req) body += chunk;
    observed = { path: req.url, body: JSON.parse(body) };
    res.on("close", () => { disconnected = true; });
    res.writeHead(200, { "Content-Type": "text/event-stream" });
    res.write(chunk({ content: "Starting to reply…" }));
    // Deliberately don't send a terminal frame: Stop must abort the open connection.
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    await open(page);
    await trigger(page).click();
    await configure(page, {}, `http://127.0.0.1:${server.address().port}`);
    await send(page, "Help me plan the afternoon");
    await expect(panel(page).getByText("Starting to reply…")).toBeVisible();
    expect(observed.path).toBe("/v1/chat/completions");
    expect(observed.body).toMatchObject({ model: "example-model", stream: true });
    await page.getByRole("button", { name: "Stop", exact: true }).click();
    await expect(panel(page).getByText("The reply was stopped.")).toBeVisible();
    await expect.poll(() => disconnected).toBe(true);
    await w.check();
  } finally {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
});

test("settings reject plain HTTP on the tailnet and HTTPS outside the allowlist", async ({ page }) => {
  const w = watch(page);
  await open(page);
  await trigger(page).click();
  await page.locator("#assistant-settings").click();
  const settings = aiSettings(page);
  await settings.getByLabel("Model", { exact: true }).fill("example-model");
  const address = settings.getByLabel("Base URL");
  const save = settings.getByRole("button", { name: "Save", exact: true });
  for (const url of ["http://model.example.ts.net", "https://model.example.ts.net", "http://100.64.0.1:11434", "https://todo.example.com"]) {
    await address.fill(url);
    await save.click();
    await expect(settings.getByRole("alert")).toContainText("Use HTTP at localhost or 127.0.0.1");
  }
  await address.fill("http://localhost:11434");
  await save.click();
  await expect(panel(page).locator(".ai-model")).toHaveText("This device · example-model");
  await w.check();
});

test("a tab can remember a scoped key; Disconnect invalidates it", async ({ page, context }) => {
  const w = watch(page);
  const apiKey = "unit-test-bearer";
  // Cookies are not scoped by port. Even an eligible loopback cookie must not be sent.
  await context.addCookies([{ name: "fixture-model-cookie", value: "synthetic-not-a-session", url: MODEL_ORIGIN }]);
  expect(await context.cookies(MODEL_ORIGIN)).toEqual(expect.arrayContaining([expect.objectContaining({ name: "fixture-model-cookie" })]));
  await open(page);
  await trigger(page).click();
  await configure(page, {}, MODEL_ORIGIN, { apiKey, remember: true });
  expect(await page.evaluate(() => Object.values(localStorage))).not.toContainEqual(expect.stringContaining(apiKey));
  await page.reload();
  await trigger(page).click();
  const requests = await modelRoute(page, () => words("Key worked."), { apiKey });
  await send(page, "Check my connection");
  await expect(panel(page).getByText("Key worked.")).toBeVisible();
  expect(JSON.stringify(requests)).not.toContain(apiKey);
  await page.locator("#assistant-settings").click();
  await expect(aiSettings(page).getByLabel("API key", { exact: true })).toHaveValue("");
  await aiSettings(page).getByRole("button", { name: "Disconnect model" }).click();
  await expect(prompt(page)).toBeDisabled();
  expect(await page.evaluate(() => Object.keys(sessionStorage).some((key) => key.startsWith("todo.assistant.api-key:")))).toBe(false);
  await w.check();
});

test("a new login in the same tab preserves the profile, not the previous login's key", async ({ page }) => {
  await open(page);
  await trigger(page).click();
  await configure(page, {}, MODEL_ORIGIN, { apiKey: "login-binding-test", remember: true });
  await page.route("**/app", async (route) => {
    const response = await route.fetch();
    await route.fulfill({ response, body: (await response.text()).replace("gallery-csrf", "another-fixture-csrf") });
  });
  await page.reload();
  await trigger(page).click();
  await expect(prompt(page)).toBeEnabled();
  await page.locator("#assistant-settings").click();
  const settings = aiSettings(page);
  await expect(settings.getByLabel("Base URL")).toHaveValue(MODEL_ORIGIN);
  await expect(settings.getByLabel("API key", { exact: true })).toHaveAttribute("placeholder", "Optional");
});

test("switching owner scopes does not load the previous owner's assistant profile or key", async ({ page }) => {
  await open(page);
  await trigger(page).click();
  await configure(page, { Inbox: "all" }, MODEL_ORIGIN, { apiKey: "account-scope-test", remember: true });
  await page.route("**/app", async (route) => {
    const response = await route.fetch();
    const body = (await response.text()).replace(/(<meta name="todo-owner-scope" content=")[a-f\d]{64}("\s*>)/, (_, before, after) => `${before}${"2".repeat(64)}${after}`);
    await route.fulfill({ response, body });
  });
  await page.reload();
  await trigger(page).click();
  await expect(prompt(page)).toBeDisabled();
  await page.locator("#assistant-settings").click();
  const settings = aiSettings(page);
  await expect(settings.getByLabel("Base URL")).toHaveValue("");
  await expect(settings.getByLabel("API key", { exact: true })).toHaveAttribute("placeholder", "Optional");
  await expect(settings.getByRole("combobox", { name: "Inbox", exact: true })).toHaveValue("hidden");
  expect(await page.evaluate(() => Object.keys(sessionStorage).some((key) => key.startsWith("todo.assistant.api-key:")))).toBe(false);
});

test("the same document restores its model on a signed-in update, without restoring its key", async ({ page }) => {
  await open(page);
  await trigger(page).click();
  await configure(page, {}, MODEL_ORIGIN, { apiKey: "signout-key-test", remember: true });
  await page.evaluate(() => { window.__sameDocument = true; });
  // A 204 holds the existing document and session, letting the live-state path see a signed-in
  // update after the sign-out gesture without changing the replica's one-way revocation latch.
  await page.route("**/auth/logout", (route) => route.fulfill({ status: 204 }));
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("menuitem", { name: "Sign out", exact: true }).click();
  await expect(prompt(page)).toBeDisabled();
  const add = page.getByRole("textbox", { name: "Add a task" });
  await add.fill("Trigger a signed-in state update");
  await add.press("Enter");
  await expect(prompt(page)).toBeEnabled();
  expect(await page.evaluate(() => window.__sameDocument)).toBe(true);
  await modelRoute(page, () => words("The model was restored."));
  await send(page, "Check restored model");
  await expect(panel(page).getByText("The model was restored.")).toBeVisible();
});

test("Loopback inference honors private-access inheritance and keeps hidden lists out of requests", async ({ page }) => {
  const w = watch(page);
  await open(page);
  await trigger(page).click();
  await configure(page, { Work: "private", Planning: "hidden" });
  await expect(panel(page).locator(".ai-model")).toHaveText("This device · example-model");
  await page.locator("#assistant-settings").click();
  const settings = aiSettings(page);
  await expect(settings.getByRole("combobox", { name: "Work", exact: true })).toHaveValue("");
  await expect(settings.getByRole("combobox", { name: "Finance", exact: true })).toHaveValue("private");
  await expect(settings.getByRole("combobox", { name: "Inbox", exact: true })).toHaveValue("hidden");
  await settings.getByRole("button", { name: "Cancel", exact: true }).click();
  const requests = await modelRoute(page, (body) => {
    switch (toolResults(body).length) {
      case 0: return call("visible-lists", "list_lists", { limit: 50 });
      case 1: return call("visible-tasks", "query_tasks", { query: "", limit: 50 });
      default: return words("I checked the permitted lists.");
    }
  });
  await send(page, "What lists and tasks can you see?");
  await expect(panel(page).getByText("I checked the permitted lists.")).toBeVisible();
  expect(requests).toHaveLength(3);
  const bodies = JSON.stringify(requests);
  expect(bodies).toContain("Finance");
  expect(bodies).toContain("Pay the December supplier invoices");
  expect(bodies).not.toContain("Planning");
  expect(bodies).not.toContain("Prepare the November board update");
  expect(bodies).not.toContain("Book train tickets to Brno");
  expect(toolResults(requests[2])[0].items.map(({ name }) => name).sort()).toEqual(["Finance", "Team"]);
  await w.check();
});
