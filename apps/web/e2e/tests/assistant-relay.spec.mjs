import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";
import { aiSettings, panel, send, words } from "./assistant-helpers.mjs";

const id = "11111111-1111-4111-8111-111111111111";
const attempt = "22222222-2222-4222-8222-222222222222";
const version = "33333333-3333-4333-8333-333333333333";
const account = () => ({ id, label: "you@example.com", approved: false, approved_lists: [] });

async function relay(context) {
  const data = { chatgpt: true, chatgpt_device_code: true, private_models: true, accounts: [], access: { lists: {}, collections: {} }, access_version: version };
  const chats = [];
  let complete = false;
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
      json = { flow: "device", attempt, user_code: "EXAMPLE-1", verification_url: "https://auth.openai.com/codex/device" };
    } else if (path === `chatgpt/poll/${attempt}`) {
      if (complete) {
        data.accounts = [account()];
        json = { state: "signed_in", account: data.accounts[0] };
      } else json = { state: "pending" };
    } else if (path === `chatgpt/accounts/${id}` && req.method() === "DELETE") {
      data.accounts = [];
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
  return { data, chats, approveSignIn: () => { complete = true; } };
}

async function settingsFor(page) {
  await open(page);
  await page.getByRole("button", { name: "Ask AI", exact: true }).click();
  await page.locator("#assistant-settings").click();
  return aiSettings(page);
}

test("device sign-in, shared account and consent, and sign-out across two browsers", async ({ browser, context, page }) => {
  const w = watch(page);
  const mock = await relay(context);
  const settings = await settingsFor(page);
  await settings.getByRole("button", { name: "Use private models" }).click();
  await settings.getByRole("button", { name: "Sign in with ChatGPT" }).click();
  await expect(settings.getByText("EXAMPLE-1", { exact: true })).toBeVisible();
  await expect(settings.getByRole("link", { name: "Open ChatGPT" })).toHaveAttribute("href", "https://auth.openai.com/codex/device");
  await expect(settings.getByRole("status").filter({ hasText: "Waiting for ChatGPT" })).toBeVisible();
  mock.approveSignIn();
  await expect(settings.getByText("you@example.com", { exact: true })).toBeVisible();
  await expect(settings.getByRole("status").filter({ hasText: "Connected you@example.com" })).toBeVisible();
  await expect(settings.getByRole("button", { name: "Add another ChatGPT account" })).toBeVisible();
  const useAccount = settings.getByRole("button", { name: "Use account" });
  await expect(useAccount).toHaveAttribute("aria-pressed", "false");
  await expect(settings.getByRole("region", { name: "Your private models (via your server)" })).toHaveAttribute("data-selected", "true");
  await useAccount.click();
  await expect(settings.getByRole("option", { name: "Example model" })).toBeAttached();
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
    await expect(aiSettings(second).getByText("you@example.com", { exact: true })).toBeVisible();
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
  await settings.getByRole("button", { name: "Sign out", exact: true }).click();
  await expect(settings.getByText("you@example.com", { exact: true })).not.toBeVisible();
  expect(mock.data.accounts).toEqual([]);
  await w.check();
});

test("private models use the paired server, not a client-held key", async ({ context, page }) => {
  const w = watch(page);
  const mock = await relay(context);
  const settings = await settingsFor(page);
  await settings.getByRole("button", { name: "Use private models" }).click();
  await expect(settings.getByLabel("API key", { exact: true })).not.toBeVisible();
  await expect(settings.getByRole("option", { name: "Example model" })).toBeAttached();
  await settings.getByRole("button", { name: "Save", exact: true }).click();
  await send(page, "Private message");
  await expect(panel(page).getByText("The private model replied.")).toBeVisible();
  expect(mock.chats[0].lists).toEqual([]);
  await w.check();
});

for (const colorScheme of ["light", "dark"]) {
  test(`assistant relay settings gallery (${colorScheme})`, async ({ context, page }) => {
    await page.emulateMedia({ colorScheme, reducedMotion: "reduce" });
    const mock = await relay(context);
    const settings = await settingsFor(page);
    await settings.getByRole("button", { name: "Sign in with ChatGPT" }).click();
    await expect(settings.getByText("EXAMPLE-1", { exact: true })).toBeVisible();
    await page.screenshot({ path: `gallery/ai-chatgpt-device-${colorScheme}.png`, animations: "disabled" });
    mock.approveSignIn();
    await expect(settings.getByRole("status").filter({ hasText: "Connected you@example.com" })).toBeVisible();
    await page.screenshot({ path: `gallery/ai-chatgpt-connected-${colorScheme}.png`, animations: "disabled" });
    await settings.getByRole("button", { name: "Use private models" }).click();
    await expect(settings.getByRole("option", { name: "Example model" })).toBeAttached();
    await page.screenshot({ path: `gallery/ai-private-relay-${colorScheme}.png`, animations: "disabled" });
  });
}
