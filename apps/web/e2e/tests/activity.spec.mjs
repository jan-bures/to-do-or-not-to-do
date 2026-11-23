import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";

const entries = (page) => page.locator(".activity-row");
const actors = (page) => page.locator(".activity-meta > span:first-child");

test("Activity lists the newest changes by day and pages when scrolled", async ({ page }) => {
  const w = watch(page);
  await open(page);
  const activity = page.getByRole("link", { name: "Activity", exact: true });
  await activity.focus();
  await activity.press("Enter");
  const heading = page.getByRole("heading", { name: "Activity" });
  await expect(heading).toBeFocused();
  await expect(page).toHaveURL(/\/app\/activity$/);
  await expect(page.getByRole("heading", { name: "Today", exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Yesterday", exact: true })).toBeVisible();
  await expect(entries(page)).toHaveCount(20);
  await expect(page.locator(".sb-counts")).toHaveText("20 shown · 4 today");
  await expect(entries(page).first()).toContainText("Completed task");
  await expect(actors(page).nth(0)).toHaveText("You");
  await expect(actors(page).nth(1)).toHaveText("Codex");
  await expect(actors(page).nth(2)).toHaveText("Email ingest");
  await expect(entries(page).first().locator("time")).toHaveText("6 min ago");
  await expect(page.getByRole("button", { name: "Filter tasks" })).toHaveCount(0);
  await expect(page.locator(".qa-input")).toHaveCount(0);
  await expect(page.getByText('Created task "Catalog inventory, shelf 1"', { exact: true })).toHaveCount(0);
  await page.locator(".room").evaluate((room) => { room.scrollTop = room.scrollHeight; });
  await expect(entries(page)).toHaveCount(31);
  await expect(page.locator(".sb-counts")).toHaveText("31 shown · 4 today");
  await expect(entries(page).last()).toContainText('Created task "Catalog inventory, shelf 1"');
  await expect(page.getByRole("heading", { name: "Thu 19.11." })).toHaveCount(1);
  await expect(page.getByRole("button", { name: "Load older activity" })).toHaveCount(0);
  await w.check();
});

test("agent links filter by actor, survive reload, and clear via browser history", async ({ page }) => {
  const w = watch(page);
  await open(page, "/app/activity");
  const agent = page.getByRole("region", { name: "Agents" }).getByRole("link", { name: /Codex/ });
  await agent.click();
  await expect(page).toHaveURL(/\/app\/activity\/agent\/codex$/);
  await expect(agent).toHaveAttribute("aria-current", "page");
  await expect(actors(page)).toHaveCount(20);
  await expect(page.locator(".sb-counts")).toHaveText("20 shown · 1 today");
  expect(new Set(await actors(page).allTextContents())).toEqual(new Set(["Codex"]));
  await page.reload();
  await expect(actors(page)).toHaveCount(20);
  await page.getByRole("link", { name: "Clear filter" }).click();
  await expect(actors(page).first()).toHaveText("You");
  await expect(page.locator(".sb-counts")).toHaveText("20 shown · 4 today");
  await page.goBack();
  await expect(actors(page).first()).toHaveText("Codex");
  await w.check();
});

test.describe("Activity's local day", () => {
  test.use({ timezoneId: "Europe/Prague" });

  test("the status rolls past midnight without reloading pages", async ({ page }) => {
    const w = watch(page);
    await page.clock.install({ time: new Date("2026-11-21T22:59:00Z") });
    await open(page, "/app/activity?fixture=browser-clock");
    await expect(page.locator(".sb-counts")).toHaveText("20 shown · 4 today");
    await page.clock.fastForward(90_000);
    await expect(page.locator(".sb-counts")).toHaveText("20 shown · 0 today");
    await expect(entries(page)).toHaveCount(20);
    await w.check();
  });
});

test("Activity distinguishes an empty log from a filter with no results", async ({ page }) => {
  const w = watch(page);
  await open(page, "/app/activity?fixture=blank");
  await expect(page.getByText("No activity yet.")).toBeVisible();
  await expect(entries(page)).toHaveCount(0);
  await open(page, "/app/activity/agent/no-matches");
  await expect(page.getByText("No activity from this agent.")).toBeVisible();
  await expect(page.getByRole("link", { name: "Clear filter" })).toBeVisible();
  await w.check();
});

test("invalid Activity URLs are replaced without adding a history entry", async ({ page }) => {
  const w = watch(page);
  await open(page);
  await open(page, "/app/activity/agent/INVALID!?fixture=blank");
  await expect(page).toHaveURL(/\/app\/activity\?fixture=blank$/);
  await expect(page.getByText("No activity yet.")).toBeVisible();
  await page.goBack();
  await expect(page.getByRole("heading", { name: "My Day" })).toBeVisible();
  await page.evaluate(() => history.pushState(null, "", "/app/activity/list/not-a-room"));
  await page.goBack();
  await page.goForward();
  await expect(page).toHaveURL(/\/app\/activity$/);
  await expect(page.getByRole("heading", { name: "Activity" })).toBeVisible();
  await w.check();
});

test("phone drawer and keyboard reach Undo and a refusal keeps its focus", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 412, height: 920 });
  await open(page);
  await page.getByRole("button", { name: "Lists", exact: true }).click();
  await page.getByRole("link", { name: "Activity", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Activity" })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(page.getByRole("button", { name: "Refresh activity" })).toBeFocused();
  const undo = page.getByRole("button", { name: /^Undo: Updated task.*due date/ });
  await undo.focus();
  await undo.press("Enter");
  const message = "Couldn't undo: the item was changed again.";
  const status = page.locator(".activity-body").getByRole("status");
  await expect(status).toHaveCount(1);
  await expect(status).toHaveText(message);
  await expect(entries(page).filter({ has: undo }).getByText(message)).toBeVisible();
  await expect(undo).toBeFocused();
  await expect(undo).toBeEnabled();
  await w.check();
});
