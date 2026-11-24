import { test, expect } from "@playwright/test";
import { open, watch } from "./helpers.mjs";

const versionPath = "**/app/version.txt";
const next = "a".repeat(64);
const footer = (page) => page.getByRole("button", { name: "Reload to update" });
const onFocus = (page) => page.evaluate(() => window.dispatchEvent(new Event("focus")));

async function deployable(page) {
  let current;
  let deployed;
  await page.route(versionPath, async (route) => {
    const response = await route.fetch();
    current = (await response.text()).trim();
    await route.fulfill({ response, body: deployed ?? current });
  });
  // Model a server that sends the new build's identity in the new document after reload.
  await page.route("**/app", async (route) => {
    if (!deployed) return route.continue();
    const response = await route.fetch();
    await route.fulfill({ response, body: (await response.text()).replace(current, deployed) });
  });
  const checked = page.waitForResponse(versionPath);
  await open(page);
  await checked;
  expect(current).toMatch(/^[a-f0-9]{64}$/);
  return () => { deployed = next; };
}

test("the sidebar offers a one-click reload when the deployed bundle changes", async ({ page }) => {
  const errors = watch(page);
  const deploy = await deployable(page);
  await expect(footer(page)).toHaveCount(0);
  deploy();
  await onFocus(page);
  await expect(footer(page)).toBeVisible();
  await expect(page.locator(".side-foot").getByRole("button", { name: "Settings" })).toBeVisible();

  const navigated = page.waitForEvent("framenavigated", (frame) => frame === page.mainFrame());
  await footer(page).click();
  await navigated;
  await expect(page.locator("meta[name=todo-web-version]")).toHaveAttribute("content", next);
  await expect(page.locator(".room-title").first()).toBeVisible();
  await expect(footer(page)).toHaveCount(0);
  await errors.check();
});

test("canceling reload leaves an unsent quick-add draft intact", async ({ page }) => {
  const deploy = await deployable(page);
  const draft = page.getByRole("textbox", { name: "Add a task" });
  await draft.fill("Unsent draft");
  deploy();
  await onFocus(page);
  await expect(footer(page)).toBeVisible();
  const dialog = page.waitForEvent("dialog");
  const click = footer(page).click();
  const warning = await dialog;
  expect(warning.message()).toContain("draft or open edit");
  await warning.dismiss();
  await click;
  await expect(draft).toHaveValue("Unsent draft");
  await expect(footer(page)).toBeVisible();
});

test("an unsaved list rename warns, but a canceled rename no longer does", async ({ page }) => {
  const deploy = await deployable(page);
  deploy();
  await onFocus(page);
  await expect(footer(page)).toBeVisible();
  await page.getByRole("link", { name: /^Finance( |$)/ }).click({ button: "right" });
  await page.getByRole("menuitem", { name: "Rename list" }).click();
  const name = page.getByRole("textbox", { name: "List name" });
  await name.fill("Unsent list name");
  const dialog = page.waitForEvent("dialog");
  // The native modal makes the sidebar inert; exercise the reload guard without
  // dismissing the edit or relying on what is currently focusable.
  const click = page.locator(".app-update-action").dispatchEvent("click");
  const warning = await dialog;
  expect(warning.message()).toContain("draft or open edit");
  await warning.dismiss();
  await click;
  await expect(name).toHaveValue("Unsent list name");
  await name.press("Escape");
  await expect(name).toHaveCount(0);
  const navigated = page.waitForEvent("framenavigated", (frame) => frame === page.mainFrame());
  await footer(page).click();
  await navigated;
});

test("reload warns about committed edits when there is no durable store", async ({ page }) => {
  const deploy = await deployable(page);
  const draft = page.getByRole("textbox", { name: "Add a task" });
  await draft.fill("Only in memory");
  await draft.press("Enter");
  await expect(draft).toHaveValue("");
  await expect(page.locator(".row").filter({ hasText: "Only in memory" })).toHaveCount(1);
  deploy();
  await onFocus(page);
  await expect(footer(page)).toBeVisible();
  const dialog = page.waitForEvent("dialog");
  const click = footer(page).click();
  const warning = await dialog;
  expect(warning.message()).toContain("In-memory changes");
  await warning.dismiss();
  await click;
  await expect(page.locator(".row").filter({ hasText: "Only in memory" })).toHaveCount(1);
});

for (const colorScheme of ["light", "dark"]) {
  test.describe(`${colorScheme} update footer gallery`, () => {
    test.use({ colorScheme, deviceScaleFactor: 2, viewport: { width: 1440, height: 900 } });

    test("the update action sits above the sync footer", async ({ page }) => {
      const deploy = await deployable(page);
      deploy();
      await onFocus(page);
      await expect(footer(page)).toBeVisible();
      const sidebar = page.locator(".sidebar");
      await expect(sidebar.locator(".side-foot .sync-text")).toHaveText(/^\d{2}:\d{2}$/);
      await sidebar.screenshot({ path: `gallery/sidebar-update-${colorScheme}-200.png`, animations: "disabled" });
    });
  });
}
