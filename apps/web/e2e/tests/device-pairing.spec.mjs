import { expect, test } from "@playwright/test";
import { open } from "./helpers.mjs";

const title = "Prepare the November board update";

test("device settings does not change task state or selection via shortcuts", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await open(page);
  const row = page.locator("[data-task-row]").filter({ hasText: title });
  await row.getByRole("button", { name: title, exact: true }).click();
  await expect(row).toHaveClass(/\bselected\b/);
  const checkbox = row.getByRole("checkbox", { includeHidden: true });
  const checked = await checkbox.getAttribute("aria-checked");
  await page.getByRole("button", { name: "Device settings" }).click();
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
  await page.getByRole("button", { name: "Device settings" }).click();
  const dialog = page.getByRole("dialog", { name: "Device settings" });
  await dialog.getByRole("button", { name: "Close", exact: true }).focus();
  await page.keyboard.press("Shift+Tab");
  await expect(dialog.getByRole("link", { name: "Devices" })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(dialog.getByRole("button", { name: "Close", exact: true })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(page.getByRole("dialog", { name: "Lists" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Device settings" })).toBeFocused();
});
