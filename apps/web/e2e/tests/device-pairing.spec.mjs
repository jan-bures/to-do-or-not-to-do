import { expect, test } from "@playwright/test";
import { open, openDeviceSettings } from "./helpers.mjs";

const title = "Prepare the November board update";

test("device settings does not change task state or selection via shortcuts", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await open(page);
  const row = page.locator("[data-task-row]").filter({ hasText: title });
  await row.getByRole("button", { name: title, exact: true }).click();
  await expect(row).toHaveClass(/\bselected\b/);
  const checkbox = row.getByRole("checkbox", { includeHidden: true });
  const checked = await checkbox.getAttribute("aria-checked");
  await openDeviceSettings(page);
  const dialog = page.getByRole("dialog", { name: "Device settings" });
  await expect(dialog).toBeVisible();
  for (const key of ["j", "k", "i", "Delete", "Control+z", "Control+k"]) await page.keyboard.press(key);
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(row).toHaveClass(/\bselected\b/);
  await expect(checkbox).toHaveAttribute("aria-checked", checked);
  await expect(page.locator(".d-title")).toHaveText(title);
});

test("phone drawer yields Tab and Escape to the device settings dialog", async ({ page }) => {
  await page.setViewportSize({ width: 412, height: 920 });
  await open(page);
  await page.getByRole("button", { name: "Lists", exact: true }).click();
  const settings = page.getByRole("button", { name: "Settings", exact: true });
  const menu = page.getByRole("menu", { name: "Settings" });
  const devices = menu.getByRole("menuitem", { name: "Devices…" });
  const admin = menu.getByRole("menuitem", { name: "Open admin pages" });
  await settings.press("Enter");
  await expect(admin).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(menu).toBeHidden();
  await expect(settings).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(admin).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  await expect(menu.getByRole("menuitem", { name: /reminder notifications/i })).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(devices).toBeFocused();
  await page.keyboard.press("Space");
  await expect(menu).toBeHidden();
  const dialog = page.getByRole("dialog", { name: "Device settings" });
  await expect(dialog.getByRole("button", { name: "Pair a new device" })).toBeFocused();
  await dialog.getByRole("button", { name: "Close", exact: true }).focus();
  await page.keyboard.press("Shift+Tab");
  await expect(dialog.getByRole("link", { name: "Devices" })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(dialog.getByRole("button", { name: "Close", exact: true })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(page.getByRole("dialog", { name: "Lists" })).toBeVisible();
  await expect(settings).toBeFocused();
});
