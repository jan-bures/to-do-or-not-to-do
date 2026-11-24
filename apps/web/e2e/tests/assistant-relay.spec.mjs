import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";
import { aiSettings, panel, send, words } from "./assistant-helpers.mjs";

const id = "11111111-1111-4111-8111-111111111111";
const attempt = "22222222-2222-4222-8222-222222222222";
const version = "33333333-3333-4333-8333-333333333333";
const account = () => ({ id, label: "you@example.com", approved: false, approved_lists: [] });
const second = { ...account(), id: "44444444-4444-4444-8444-444444444444", label: "second@example.com" };
const segment = (settings, name) => settings.getByRole("group", { name: "Model provider" }).getByRole("button", { name, exact: true });
const accountRow = (settings, label) => settings.getByRole("button", { name: label, exact: true });

async function relay(context) {
  const data = { chatgpt: true, chatgpt_device_code: true, private_models: true, accounts: [], access: { lists: {}, collections: {} }, access_version: version };
  const chats = [];
  let completedAccount;
  await context.route("**/api/v1/assistant/**", async (route) => {
    const req = route.request();
    const headers = await req.allHeaders();
    expect(headers["x-csrf-token"]).toBe("gallery-csrf");
    expect(headers).not.toHaveProperty("authorization");
    const path = new URL(req.url()).pathname.replace("/api/v1/assistant/", "");
    let json;
    if (path === "status") json = data;
    else if (path === "chatgpt/start") {
      expect(req.postDataJSON()).toEqual({ flow: "device" });
      completedAccount = undefined;
      json = { flow: "device", attempt, user_code: "EXAMPLE-1", verification_url: "https://auth.openai.com/codex/device" };
    } else if (path === `chatgpt/poll/${attempt}`) {
      if (completedAccount) {
        data.accounts = [...data.accounts.filter((item) => item.id !== completedAccount.id), completedAccount];
        json = { state: "signed_in", account: completedAccount };
      } else json = { state: "pending" };
    } else if (path.startsWith("chatgpt/accounts/") && req.method() === "DELETE") {
      data.accounts = data.accounts.filter((item) => item.id !== path.split("/").at(-1));
      json = { revoked: true };
    } else if (path === "access") {
      const input = req.postDataJSON();
      expect(input.access_version).toBe(version);
      data.access = input.access;
      json = data;
    } else if (path === `chatgpt/accounts/${id}/consent`) {
      const input = req.postDataJSON();
      expect(input.access_version).toBe(version);
      json = data.accounts[0] = { ...account(), approved: true, approved_lists: input.lists };
    } else if (path.endsWith("/models")) {
      json = [{ id: "fixture-model", label: "Example model" }];
    } else if (path.endsWith("/chat")) {
      const body = req.postDataJSON();
      chats.push(body);
      let response;
      if (path.startsWith("chatgpt/")) {
        expect(body.destination).toBe("cloud");
        expect(body.request.store).toBe(false);
        response = `data: ${JSON.stringify({ type: "response.output_text.delta", delta: "ChatGPT replied." })}\n\ndata: {"type":"response.completed"}\n\n`;
      } else {
        expect(body.destination).toBe("local");
        response = words("The private model replied.");
      }
      return route.fulfill({ contentType: "text/event-stream", body: response });
    } else throw new Error(`Unexpected assistant route: ${path}`);
    await route.fulfill({ json });
  });
  return { data, chats, approveSignIn: (item = account()) => { completedAccount = item; } };
}

async function settingsFor(page) {
  await open(page);
  await page.getByRole("button", { name: "Ask AI", exact: true }).click();
  await page.locator("#assistant-settings").click();
  return aiSettings(page);
}

async function chooseModel(settings) {
  await settings.getByRole("button", { name: "Choose model" }).click();
  await settings.getByRole("combobox", { name: "Model", exact: true }).selectOption("fixture-model");
}

test("device sign-in, shared account and consent, and sign-out across two browsers", async ({ browser, context, page }) => {
  const w = watch(page);
  const mock = await relay(context);
  const settings = await settingsFor(page);
  await segment(settings, "ChatGPT").click();
  await settings.getByRole("button", { name: "Sign in with ChatGPT" }).click();
  await expect(settings.getByText("EXAMPLE-1", { exact: true })).toBeVisible();
  await expect(settings.getByRole("link", { name: "Open ChatGPT" })).toHaveAttribute("href", "https://auth.openai.com/codex/device");
  await expect(settings.getByRole("status").filter({ hasText: "Waiting for ChatGPT" })).toBeVisible();
  mock.approveSignIn();
  await expect(accountRow(settings, account().label)).toBeVisible();
  const connected = settings.getByRole("status").filter({ hasText: "Connected you@example.com." });
  await expect(connected).toBeVisible();
  await expect(settings.getByRole("button", { name: "Add another account" })).toBeVisible();
  const savedAccount = accountRow(settings, account().label);
  await expect(savedAccount).toHaveAttribute("aria-pressed", "true");
  const usage = settings.getByRole("link", { name: "Manage usage" });
  const usageLeft = await usage.evaluate((link) => {
    const label = document.createRange();
    label.selectNodeContents(link.firstChild);
    return label.getBoundingClientRect().left;
  });
  const accountBox = await savedAccount.boundingBox();
  const connectedBox = await connected.boundingBox();
  expect(connectedBox.y).toBeGreaterThan(accountBox.y);
  expect(usageLeft).toBe(accountBox.x);
  expect(usageLeft).toBe(connectedBox.x);
  await expect(settings.getByRole("button", { name: "Choose model" })).toBeEnabled();
  await expect(settings.getByText("No model selected", { exact: true })).toBeVisible();
  await expect(settings.getByRole("button", { name: "Save", exact: true })).toBeDisabled();
  const model = settings.getByRole("region", { name: "ChatGPT", exact: true }).getByText("Model", { exact: true });
  expect(usageLeft).toBe((await model.boundingBox()).x);
  await chooseModel(settings);
  await settings.getByRole("combobox", { name: "Planning", exact: true }).selectOption("all");
  await settings.getByRole("combobox", { name: "Inbox", exact: true }).selectOption("private");
  await settings.getByRole("button", { name: "Save", exact: true }).click();
  await send(page, "First cloud message");
  await expect(panel(page).getByText("Share with ChatGPT?")).toBeVisible();
  expect(mock.chats).toHaveLength(0);
  await expect(panel(page).getByText(/ChatGPT can read these lists: Planning\./)).toBeVisible();
  await panel(page).getByRole("button", { name: "Allow and send" }).click();
  await expect(panel(page).getByText("ChatGPT replied.")).toBeVisible();
  expect(mock.chats[0].lists).toHaveLength(1);
  const secondContext = await browser.newContext();
  try {
    await secondContext.route("**/api/v1/assistant/**", (route) => route.fulfill({ json: mock.data }));
    const second = await secondContext.newPage();
    await second.goto(page.url());
    await second.getByRole("button", { name: "Ask AI", exact: true }).click();
    await second.locator("#assistant-settings").click();
    await segment(aiSettings(second), "ChatGPT").click();
    await expect(accountRow(aiSettings(second), account().label)).toBeVisible();
  } finally { await secondContext.close(); }
  await page.locator("#assistant-settings").click();
  const finance = settings.getByRole("combobox", { name: "Finance", exact: true });
  await expect(finance).toHaveValue("hidden");
  await finance.selectOption("all");
  await expect(finance).toHaveValue("all");
  await settings.getByRole("button", { name: "Save", exact: true }).click();
  await expect(settings).not.toBeVisible();
  expect(Object.values(mock.data.access.lists).filter((access) => access === "All")).toHaveLength(2);
  await send(page, "Scope grew");
  await expect(panel(page).getByText(/ChatGPT can read these lists:.*Finance/)).toBeVisible();
  expect(mock.chats).toHaveLength(1);
  await page.locator("#assistant-settings").click();
  await settings.getByRole("button", { name: "Sign out of you@example.com", exact: true }).click();
  await expect(settings.getByText("you@example.com", { exact: true })).not.toBeVisible();
  expect(mock.data.accounts).toEqual([]);
  await w.check();
});

test("private models use the paired server, not a client-held key", async ({ context, page }) => {
  const w = watch(page);
  const mock = await relay(context);
  const settings = await settingsFor(page);
  await settings.getByLabel("Base URL").fill("not a URL");
  await segment(settings, "Your private models").click();
  await expect(settings.getByLabel("API key", { exact: true })).not.toBeVisible();
  await expect(settings.getByRole("button", { name: "Choose model" })).toBeEnabled();
  await chooseModel(settings);
  await settings.getByRole("button", { name: "Save", exact: true }).click();
  await expect(settings).not.toBeVisible();
  await expect(page.locator("#assistant-settings")).toBeFocused();
  await send(page, "Private message");
  await expect(panel(page).getByText("The private model replied.")).toBeVisible();
  expect(mock.chats[0].lists).toEqual([]);
  await page.locator("#assistant-settings").click();
  await segment(settings, "This device").click();
  await expect(settings.getByLabel("Base URL")).toHaveValue("");
  await w.check();
});

test("sign-in prefers the new account only if the current choice is missing", async ({ context, page }) => {
  const mock = await relay(context);
  mock.data.accounts = [account()];
  const settings = await settingsFor(page);
  await segment(settings, "ChatGPT").click();
  await settings.getByRole("button", { name: "Add another account" }).click();
  await expect(settings.getByText("EXAMPLE-1", { exact: true })).toBeVisible();
  mock.approveSignIn(second);
  await expect(accountRow(settings, second.label)).toHaveAttribute("aria-pressed", "true");
  await expect(accountRow(settings, account().label)).toHaveAttribute("aria-pressed", "false");
  await chooseModel(settings);
  await settings.getByRole("button", { name: "Add another account" }).click();
  await expect(settings.getByText("EXAMPLE-1", { exact: true })).toBeVisible();
  mock.approveSignIn(account());
  await expect(settings.getByRole("status").filter({ hasText: "Connected you@example.com." })).toBeVisible();
  await expect(accountRow(settings, second.label)).toHaveAttribute("aria-pressed", "true");
  await expect(settings.getByText("fixture-model", { exact: true })).toBeVisible();

  await settings.getByRole("button", { name: "Save", exact: true }).click();
  await expect(settings).not.toBeVisible();
  mock.data.accounts = [account()];
  await page.locator("#assistant-settings").click();
  await expect(accountRow(settings, account().label)).toHaveAttribute("aria-pressed", "true");
  await expect(settings.getByText("No model selected", { exact: true })).toBeVisible();
});

test("sign-in preserves a private model selection", async ({ context, page }) => {
  const mock = await relay(context);
  const settings = await settingsFor(page);
  await segment(settings, "Your private models").click();
  await chooseModel(settings);
  await segment(settings, "ChatGPT").click();
  await settings.getByRole("button", { name: "Sign in with ChatGPT" }).click();
  await expect(settings.getByText("EXAMPLE-1", { exact: true })).toBeVisible();
  mock.approveSignIn();
  await expect(settings.getByRole("status").filter({ hasText: "Connected you@example.com." })).toBeVisible();
  await expect(accountRow(settings, account().label)).toHaveAttribute("aria-pressed", "false");
  await segment(settings, "Your private models").click();
  await expect(settings.getByText("fixture-model", { exact: true })).toBeVisible();
});

test("provider choices, active account, and capability-specific sign-in fit a narrow screen", async ({ context, page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 320, height: 700 });
  const mock = await relay(context);
  mock.data.chatgpt_device_code = false;
  const long = `${"long-".repeat(20)}account@example.com`;
  mock.data.accounts = [account(), { ...second, label: long }];
  await open(page);
  await page.getByRole("button", { name: "Ask AI", exact: true }).click();
  const empty = panel(page).locator(".ai-empty .ai-option");
  await expect(empty).toHaveText(["ChatGPT", "Your private models", "This device"]);
  await empty.first().click();
  const settings = aiSettings(page);
  await expect(segment(settings, "ChatGPT")).toHaveAttribute("aria-pressed", "true");
  await expect(settings.locator(".ai-provider:visible")).toHaveCount(1);
  await expect(settings.getByText("Sign in from the desktop or Android app. The account then works here too.")).toBeVisible();
  await expect(settings.getByRole("button", { name: /Sign in with ChatGPT|Add another account|Use a code instead/ })).toHaveCount(0);
  const first = accountRow(settings, account().label);
  const other = accountRow(settings, long);
  await first.click();
  await expect(first).toHaveAttribute("aria-pressed", "true");
  await segment(settings, "This device").click();
  await expect(settings.getByRole("region", { name: "This device" })).toBeVisible();
  await segment(settings, "ChatGPT").click();
  await expect(first).toHaveAttribute("aria-pressed", "true");
  await other.click();
  await expect(first).toHaveAttribute("aria-pressed", "false");
  await expect(other).toHaveAttribute("aria-pressed", "true");
  const truncated = other.locator(".ai-truncate");
  await expect(truncated).toHaveCSS("white-space", "nowrap");
  expect(await truncated.evaluate((el) => el.scrollWidth > el.clientWidth)).toBe(true);
  await settings.getByRole("button", { name: `Sign out of ${long}` }).click();
  await expect(other).toHaveCount(0);
  await expect(first).toHaveAttribute("aria-pressed", "true");
  await expect(settings.getByText("No model selected", { exact: true })).toBeVisible();
  await expect(settings.getByRole("button", { name: "Save", exact: true })).toBeDisabled();
  expect(mock.data.accounts).toEqual([account()]);
  await w.check();
});

for (const colorScheme of ["light", "dark"]) {
  test(`assistant relay settings gallery (${colorScheme})`, async ({ context, page }) => {
    await page.emulateMedia({ colorScheme, reducedMotion: "reduce" });
    const mock = await relay(context);
    await page.setViewportSize({ width: 1440, height: 900 });
    await open(page);
    await page.getByRole("button", { name: "Ask AI", exact: true }).click();
    const empty = panel(page).locator(".ai-empty .ai-option");
    await expect(empty).toHaveText(["ChatGPT", "Your private models", "This device"]);
    await page.screenshot({ path: `gallery/ai-connect-providers-${colorScheme}.png`, animations: "disabled" });
    await empty.first().click();
    const settings = aiSettings(page);
    await settings.getByRole("button", { name: "Sign in with ChatGPT" }).click();
    await expect(settings.getByText("EXAMPLE-1", { exact: true })).toBeVisible();
    await page.screenshot({ path: `gallery/ai-chatgpt-device-${colorScheme}.png`, animations: "disabled" });
    mock.approveSignIn();
    await expect(settings.getByRole("status").filter({ hasText: "Connected you@example.com" })).toBeVisible();
    await page.screenshot({ path: `gallery/ai-chatgpt-confirmed-${colorScheme}.png`, animations: "disabled" });
    await expect(settings.getByRole("button", { name: "Choose model" })).toBeEnabled();
    await page.screenshot({ path: `gallery/ai-chatgpt-account-${colorScheme}.png`, animations: "disabled" });
    await chooseModel(settings);
    await settings.getByRole("button", { name: "Save", exact: true }).click();
    await expect(settings).not.toBeVisible();
    mock.data.accounts.push(second);
    mock.data.chatgpt_device_code = false;
    await page.locator("#assistant-settings").click();
    await expect(accountRow(settings, second.label)).toBeVisible();
    await page.screenshot({ path: `gallery/ai-chatgpt-connected-${colorScheme}.png`, animations: "disabled" });
    await page.setViewportSize({ width: 360, height: 780 });
    await page.screenshot({ path: `gallery/ai-chatgpt-connected-phone-${colorScheme}.png`, animations: "disabled" });
    await page.setViewportSize({ width: 1440, height: 900 });
    const privateSegment = segment(settings, "Your private models");
    await privateSegment.click();
    await expect(settings.getByRole("button", { name: "Choose model" })).toBeEnabled();
    await expect(privateSegment).toHaveCSS("background-color", await privateSegment.evaluate((el) => getComputedStyle(el).borderTopColor));
    await page.screenshot({ path: `gallery/ai-private-relay-${colorScheme}.png`, animations: "disabled" });
  });
}
