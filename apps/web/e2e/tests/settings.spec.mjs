import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";

for (const [layout, viewport, key] of [
  ["desktop", { width: 1440, height: 900 }, "Enter"],
  ["phone", { width: 412, height: 920 }, "Space"],
]) {
  test(`${layout} settings opens the current server's admin pages in a new tab`, async ({ page, context }) => {
    const w = watch(page);
    await page.setViewportSize(viewport);
    await open(page, "/app/important");
    const origin = new URL(page.url()).origin;
    await context.route(`${origin}/admin`, (route) => route.fulfill({ body: "Admin pages" }));
    if (layout === "phone") await page.getByRole("button", { name: "Lists", exact: true }).click();
    const gear = page.getByRole("button", { name: "Settings", exact: true });
    const menu = page.getByRole("menu", { name: "Settings" });
    const admin = menu.getByRole("menuitem", { name: "Open admin pages", exact: true });
    const sync = menu.getByRole("menuitem", { name: "Sync now", exact: true });
    await gear.press("Enter");
    await expect(menu.getByText(origin, { exact: true })).toBeVisible();
    await expect(admin).toBeFocused();
    await page.keyboard.press("ArrowDown");
    await expect(sync).toBeFocused();
    await expect(sync).toBeDisabled(); // Fixture data has no sync engine.
    await page.keyboard.press("Enter");
    await expect(menu).toBeVisible();
    await page.keyboard.press("ArrowUp");
    await expect(admin).toBeFocused();
    const opened = page.waitForEvent("popup");
    await page.keyboard.press(key);
    const popup = await opened;
    await expect(popup).toHaveURL(`${origin}/admin`);
    await expect(page).toHaveURL(`${origin}/app/important`);
    await popup.close();
    await expect(gear).toBeFocused();
    await expect(menu).toBeHidden();
    await w.check();
  });
}
