import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";
import { aiSettings, call, configure, modelRoute, panel, send, toolResults, words } from "./assistant-helpers.mjs";

const WIDE = { width: 1440, height: 900 };
const MEDIUM = { width: 1024, height: 768 };
const NARROW = { width: 390, height: 844 };
const LONG_TITLE = "Vyřídit žádost o přeplatek na zdravotním pojištění a doložit potvrzení";
const shot = (page, name, theme) => page.screenshot({ path: `gallery/ai-${name}-${theme}.png`, animations: "disabled" });

for (const theme of ["light", "dark"]) {
  test.describe(`${theme} assistant gallery`, () => {
    test.use({ colorScheme: theme, deviceScaleFactor: 2, viewport: WIDE });

    test("unconfigured and empty wide, settings medium, creation sheet narrow", async ({ page }) => {
      const w = watch(page);
      await open(page);
      await page.getByRole("button", { name: "Ask AI", exact: true }).click();
      await expect(panel(page).getByText("Connect a model")).toBeVisible();
      await shot(page, "wide-unconfigured", theme);
      await configure(page, { Inbox: "all", Work: "private", Planning: "all" });
      await expect(panel(page).getByText("What can I help plan?")).toBeVisible();
      await shot(page, "wide-empty", theme);

      await page.setViewportSize(MEDIUM);
      await expect(panel(page)).toHaveAttribute("aria-modal", "true");
      await page.locator("#assistant-settings").click();
      await expect(aiSettings(page)).toBeVisible();
      await expect(aiSettings(page).getByRole("heading", { name: "Your own server" })).toBeInViewport();
      await shot(page, "medium-settings", theme);
      await page.keyboard.press("Escape");
      await expect(aiSettings(page)).not.toBeVisible();

      await page.setViewportSize(NARROW);
      await modelRoute(page, (body) => toolResults(body).length === 0
        ? call("new-task", "create_task", { title: LONG_TITLE, due: "tomorrow", priority: "high" })
        : words("I drafted one task for tomorrow. Review the details before adding it."));
      await send(page, "Turn this note into a task for tomorrow");
      const card = panel(page).getByRole("region", { name: /^New task:/ });
      await expect(card.getByRole("button", { name: "Approve", exact: true })).toBeVisible();
      await expect(card.getByText(LONG_TITLE)).toBeVisible();
      await shot(page, "narrow-proposal", theme);
      await w.check();
    });

    test("wide edit and medium batch", async ({ page }) => {
      const w = watch(page);
      await open(page);
      await page.getByRole("button", { name: "Ask AI", exact: true }).click();
      await configure(page, { Inbox: "all", Planning: "all" });
      await modelRoute(page, (body) => {
        const results = toolResults(body);
        const prompt = body.messages.filter((m) => m.role === "user").at(-1).content;
        if (prompt === "Move the board update to Monday") {
          if (results.length === 0) return call("find", "query_tasks", { query: "November board update" });
          if (results.length === 1) return call("edit", "update_task", { task_id: results[0].items[0].id, due: "2026-11-23" });
          return words("I’ve prepared a due date change. Nothing has been updated yet.");
        }
        if (results.length === 0) return call("first", "create_task", { title: "Pick up oat milk and lemons", due: "tomorrow" });
        if (results.length === 1) return call("second", "create_task", { title: LONG_TITLE, priority: "high" });
        if (results.length === 2) return call("third", "create_task", { title: "Send the workshop handout" });
        return words("Here are three new tasks to review.");
      });
      await send(page, "Move the board update to Monday");
      await expect(panel(page).getByRole("region", { name: /^Edit task:/ })).toBeVisible();
      await shot(page, "wide-edit", theme);

      await page.getByRole("button", { name: "New chat", exact: true }).click();
      await page.setViewportSize(MEDIUM);
      await send(page, "Draft three tasks from my note");
      const batch = panel(page).getByRole("region", { name: "3 proposed changes" });
      await expect(batch).toBeVisible();
      await batch.getByRole("checkbox", { name: `Include ${LONG_TITLE}` }).uncheck();
      await expect(batch.getByRole("button", { name: "Approve 2", exact: true })).toBeVisible();
      await expect(batch.locator(".ai-batch-title").filter({ hasText: LONG_TITLE })).toHaveCSS("text-overflow", "ellipsis");
      await shot(page, "medium-batch", theme);
      await w.check();
    });
  });
}
