import { expect } from "@playwright/test";

const ORIGIN = "https://todo.example.com";
export const panel = (page) => page.locator("#assistant-panel");
export const prompt = (page) => page.getByRole("textbox", { name: "Message the assistant" });
export const aiSettings = (page) => page.getByRole("dialog", { name: "AI assistant", exact: true }).filter({ has: page.getByLabel("Base URL") });

export async function configure(page, rules = {}, address = ORIGIN) {
  await page.locator("#assistant-settings").click();
  const settings = aiSettings(page);
  await settings.getByLabel("Base URL").fill(address);
  await settings.getByLabel("Model", { exact: true }).fill("example-model");
  if (address.startsWith("https:")) {
    await settings.getByRole("checkbox", { name: /send prompts/i }).check();
  }
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

export async function modelRoute(page, reply) {
  const requests = [];
  await page.route(`${ORIGIN}/v1/chat/completions`, async (route) => {
    const r = route.request();
    const headers = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type", "access-control-allow-methods": "POST, OPTIONS" };
    if (r.method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers });
      return;
    }
    expect(r.method()).toBe("POST");
    expect(r.headers()).not.toHaveProperty("cookie");
    expect(r.headers()).not.toHaveProperty("authorization");
    expect(r.headers()).not.toHaveProperty("x-csrf-token");
    expect(r.headers()).not.toHaveProperty("referer");
    const body = r.postDataJSON();
    requests.push(body);
    await route.fulfill({ status: 200, contentType: "text/event-stream", headers, body: reply(body) });
  });
  return requests;
}
