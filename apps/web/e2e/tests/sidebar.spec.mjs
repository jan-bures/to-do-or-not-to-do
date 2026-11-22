import { readFileSync } from "node:fs";
import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";

const tokens = JSON.parse(readFileSync(new URL("../../../../crates/todo-design/generated/tokens.json", import.meta.url)));
const { desktop, component } = tokens.metrics;
const rgb = (hex) => `rgb(${hex.slice(1).match(/../g).map((pair) => parseInt(pair, 16)).join(", ")})`;
const sidebar = (page) => page.getByRole("complementary", { name: "Lists" });
const navLink = (page, path) => sidebar(page).locator(`a[href="/app${path}"]`);

for (const scheme of ["light", "dark"]) {
  test(`smart-list and Activity icons keep their glyph colours (${scheme})`, async ({ page }) => {
    const w = watch(page);
    await page.emulateMedia({ colorScheme: scheme });
    await page.setViewportSize({ width: 1440, height: 900 });
    await open(page);
    for (const [path, hue] of [
      ["", "my-day"], ["/important", "important"], ["/planned", "planned"],
      ["/all", "all"], ["/completed", "completed"], ["/agent-activity", "agent-activity"],
      ["/activity", "agent-activity"],
    ]) {
      const link = navLink(page, path);
      const glyph = rgb(tokens.colors[scheme].lists[hue].glyph);
      await expect(link.locator(".icon svg")).toHaveCSS("stroke", glyph);
      await link.hover();
      await expect(link.locator(".icon svg")).toHaveCSS("stroke", glyph);
      await link.click();
      await expect(link).toHaveAttribute("aria-current", "page");
      await expect(link.locator(".icon svg")).toHaveCSS("stroke", glyph);
    }
    await w.check();
  });
}

test("collection headers have their own glyphs, desktop spacing and a visible zero", async ({ page }) => {
  const w = watch(page);
  await page.emulateMedia({ colorScheme: "light" });
  await page.setViewportSize({ width: 1440, height: 900 });
  await open(page);
  const nav = sidebar(page);
  const work = nav.getByRole("link", { name: /^Work / });
  const home = nav.getByRole("link", { name: /^Osobní / });
  const planning = nav.getByRole("link", { name: /^Planning / });
  const smart = navLink(page, "/important");
  const toggle = nav.getByRole("button", { name: "Collapse Work" });
  await expect(work.locator(".glyph")).toHaveAttribute("data-glyph", "briefcase");
  await expect(home.locator(".glyph")).toHaveAttribute("data-glyph", "house");
  await expect(work).toHaveCSS("height", `${desktop["nav-sub-row"]}px`);
  await expect(smart).toHaveCSS("height", `${desktop["nav-row"]}px`);
  const hue = await planning.getAttribute("data-list-hue");
  await expect(planning.locator(".glyph svg")).toHaveCSS("stroke", rgb(tokens.colors.light.lists[hue].glyph));
  const workBox = await work.boundingBox();
  const planningBox = await planning.boundingBox();
  const homeBox = await home.boundingBox();
  expect(workBox.y + workBox.height).toBe(planningBox.y);
  expect(homeBox.y - planningBox.y - 3 * planningBox.height).toBe(tokens.space[2]);
  const chevron = await toggle.locator(".icon").boundingBox();
  const workGlyph = await work.locator(".glyph").boundingBox();
  const childGlyph = await planning.locator(".glyph").boundingBox();
  expect(chevron.width).toBe(component["icon-small"]);
  expect(workGlyph.x - chevron.x - chevron.width).toBe(tokens.space[2]);
  expect(childGlyph.x - workBox.x).toBe(tokens.space[6] + tokens.space[1] + tokens.elevation.hairline);
  const right = async (item) => {
    const { x, width } = await item.locator(".count").boundingBox();
    return x + width;
  };
  expect(await right(work)).toBe(await right(planning));
  expect(await right(work)).toBe(await right(smart));
  await toggle.click();
  await expect(planning).toBeHidden();
  expect((await home.boundingBox()).y - workBox.y).toBe(workBox.height);

  const create = nav.getByRole("button", { name: "New collection", exact: true });
  await expect(create).toHaveCSS("height", `${desktop["nav-sub-row"]}px`);
  await create.click();
  const input = nav.getByRole("textbox", { name: "Collection name" });
  await input.fill("Weekend");
  await input.press("Enter");
  const empty = nav.getByRole("link", { name: "Weekend 0", exact: true });
  await expect(empty.locator(".glyph")).toHaveAttribute("data-glyph", "layers");
  await expect(empty.locator(".count")).toHaveText("0");
  await expect(empty.locator(".count")).toHaveCSS("opacity", "1");
  await expect(empty).toHaveCSS("border-top-color", rgb(tokens.colors.light.rooms[
    await empty.getAttribute("data-list-hue")
  ].line));
  await w.check();
});

test("hover preserves the selected surface and overflow keeps the count accessible", async ({ page }) => {
  const w = watch(page);
  await page.emulateMedia({ colorScheme: "light" });
  await page.setViewportSize({ width: 1440, height: 900 });
  await open(page);
  const nav = sidebar(page);
  const planning = nav.getByRole("link", { name: "Planning 2", exact: true });
  const menu = nav.getByRole("button", { name: "Menu for Planning" });
  const count = planning.locator(".count");
  await expect(menu).toHaveCSS("opacity", "0");
  await expect(count).toHaveCSS("opacity", "1");
  await planning.hover();
  await expect(planning).toHaveCSS("background-color", rgb(tokens.colors.light.rooms["my-day"].hover));
  await expect(menu).toHaveCSS("opacity", "1");
  await expect(count).toHaveCSS("opacity", "0");
  await expect(planning).toHaveAccessibleName("Planning 2");
  await page.locator(".brand").hover();
  await planning.focus();
  await expect(menu).toHaveCSS("opacity", "1");
  await expect(count).toHaveCSS("opacity", "0");
  await expect(planning).toHaveAccessibleName("Planning 2");

  await planning.click();
  await expect(planning).toHaveAttribute("aria-current", "page");
  const hue = await planning.getAttribute("data-list-hue");
  const raised = rgb(tokens.colors.light.rooms[hue].raised);
  await planning.hover();
  await expect(planning).toHaveCSS("background-color", raised);

  const toggle = nav.getByRole("button", { name: "Collapse Work" });
  await toggle.click();
  const work = nav.getByRole("link", { name: /^Work / });
  await expect(work).toHaveClass(/contains-active/);
  await work.hover();
  await expect(work).toHaveCSS("background-color", raised);
  await w.check();
});

test("Activity sits above today's indented agent filters and omits the group on a quiet day", async ({ page }) => {
  const w = watch(page);
  await open(page);
  const nav = sidebar(page);
  const activity = navLink(page, "/activity");
  const agents = nav.getByRole("region", { name: "Agents" });
  await expect(navLink(page, "/agent-activity")).toHaveText(/Agent activity\s*2 new/);
  await expect(nav.locator('a[href="/app/agent-activity"] + a[href="/app/activity"] + .side-agents')).toHaveCount(1);
  await expect(agents.getByRole("link")).toHaveCount(2);
  await expect(agents.getByRole("link", { name: /Pi/ })).toHaveCount(0);
  const codex = agents.getByRole("link", { name: /Codex/ });
  await expect(codex.locator(".count")).toHaveText("1 edited");
  expect((await codex.locator(".mark").boundingBox()).x).toBeGreaterThan((await activity.locator(".icon").boundingBox()).x);
  await codex.click();
  await expect(page).toHaveURL(/\/app\/activity\/agent\/codex$/);
  await expect(codex).toHaveAttribute("aria-current", "page");
  await open(page, "/app?fixture=blank");
  await expect(activity).toBeVisible();
  await expect(agents).toHaveCount(0);
  await w.check();
});

test("creation actions stay pinned when the navigation scrolls", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 1200, height: 420 });
  await open(page);
  const nav = sidebar(page).locator("nav");
  const footer = sidebar(page).getByRole("button", { name: "New collection", exact: true });
  const before = await footer.boundingBox();
  const scrolled = await nav.evaluate((el) => (el.scrollTop = el.scrollHeight, el.scrollTop));
  expect(scrolled).toBeGreaterThan(0);
  expect(await footer.boundingBox()).toEqual(before);
  await expect(footer).toBeInViewport();
  await w.check();
});

for (const width of [1440, 412]) {
  test(`sync and sign-out stay inside the short sidebar with its form open (${width}px)`, async ({ page }) => {
    const w = watch(page);
    await page.setViewportSize({ width, height: 200 });
    await open(page);
    if (width < 760) await page.getByRole("button", { name: "Lists", exact: true }).click();
    const drawer = page.locator(".sidebar");
    await drawer.getByRole("button", { name: "New list", exact: true }).click();
    const input = drawer.getByRole("textbox", { name: "List name", exact: true });
    await expect(input).toBeFocused();
    await expect(input).toBeInViewport({ ratio: 1 });
    const bounds = await drawer.boundingBox();
    const footer = await drawer.locator(".side-foot").boundingBox();
    expect(footer.y).toBeGreaterThanOrEqual(bounds.y);
    expect(footer.y + footer.height).toBeLessThanOrEqual(bounds.y + bounds.height);
    await expect(drawer.getByRole("button", { name: "Sign out" })).toBeInViewport({ ratio: 1 });
    await w.check();
  });
}
