import { expect } from "@playwright/test";

export const MODEL_ORIGIN = "http://127.0.0.1:11434";
export const panel = (page) => page.locator("#assistant-panel");
export const prompt = (page) => page.getByRole("textbox", { name: "Message the assistant" });
export const aiSettings = (page) => page.locator("#assistant-settings-dialog");

export async function configure(page, rules = {}, address = MODEL_ORIGIN, { apiKey, remember = false } = {}) {
  await page.locator("#assistant-settings").click();
  const settings = aiSettings(page);
  await settings.getByLabel("Base URL").fill(address);
  await settings.getByLabel("Model", { exact: true }).fill("example-model");
  if (apiKey) await settings.getByLabel("API key", { exact: true }).fill(apiKey);
  if (remember) await settings.getByRole("checkbox", { name: "Remember in this tab" }).check();
  for (const [name, access] of Object.entries(rules)) {
    await settings.getByRole("combobox", { name, exact: true }).selectOption(access);
  }
  await settings.getByRole("button", { name: "Save", exact: true }).click();
  await expect(settings).not.toBeVisible();
  await expect(prompt(page)).toBeEnabled();
}

export async function send(page, text) {
  await prompt(page).fill(text);
  await prompt(page).press("Enter");
}

export const chunk = (delta, finish_reason = null) => `data: ${JSON.stringify({ choices: [{ index: 0, delta, finish_reason }] })}\n\n`;
const done = "data: [DONE]\n\n";
export const words = (content) => chunk({ content }) + chunk({}, "stop") + done;
export const call = (id, name, args) => chunk({ tool_calls: [{ index: 0, id, type: "function", function: { name, arguments: JSON.stringify(args) } }] }) + chunk({}, "tool_calls") + done;
export const toolResults = (req) => req.messages.filter((m) => m.role === "tool").map((m) => JSON.parse(m.content));

export async function modelRoute(page, reply, { apiKey, origin = MODEL_ORIGIN } = {}) {
  const requests = [];
  await page.route(`${origin}/v1/chat/completions`, async (route) => {
    const r = route.request();
    const headers = { "access-control-allow-origin": "*", "access-control-allow-headers": "authorization, content-type", "access-control-allow-methods": "POST, OPTIONS" };
    if (r.method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers });
      return;
    }
    expect(r.method()).toBe("POST");
    const sent = await r.allHeaders();
    expect(sent).not.toHaveProperty("cookie");
    if (apiKey) expect(sent.authorization).toBe(`Bearer ${apiKey}`);
    else expect(sent).not.toHaveProperty("authorization");
    expect(sent).not.toHaveProperty("x-csrf-token");
    expect(sent).not.toHaveProperty("referer");
    const body = r.postDataJSON();
    requests.push(body);
    await route.fulfill({ status: 200, contentType: "text/event-stream", headers, body: reply(body) });
  });
  return requests;
}
