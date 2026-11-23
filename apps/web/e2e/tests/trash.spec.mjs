import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";

const paint = "Return the paint samples";
const draft = "Send the first draft to print";
const restore = (page, title) => page.getByRole("button", { name: `Restore ${title}`, exact: true });

test("Trash restores tasks to their list, and Undo restores the same tombstone", async ({ page }) => {
  const w = watch(page);
  await open(page, "/app?fixture=trash");
  await page.getByRole("link", { name: "Trash", exact: true }).click();
  await expect(page).toHaveURL(/\/app\/trash\?fixture=trash$/);
  await expect(page.getByRole("heading", { name: "Trash", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Filter tasks" })).toHaveCount(0);
  await expect(page.locator(".trash-title")).toHaveText([paint, "Spring fair", "Flyers", draft]);
  const metadata = page.getByText("Task · Deleted 14 min ago · from Garden", { exact: true });
  await expect(metadata).toBeVisible();
  await restore(page, paint).press("Enter");
  await expect(page.getByText(paint, { exact: true })).toHaveCount(0);
  await expect(restore(page, "Spring fair")).toBeFocused();
  await page.locator(".toast").getByRole("button", { name: "Undo" }).click();
  await expect(metadata).toBeVisible();
  await restore(page, paint).click();
  await page.getByRole("link", { name: /^Garden\b/ }).click();
  await expect(page.getByRole("button", { name: paint, exact: true })).toBeVisible();
  await w.check();
});

test("phone drawer opens Trash; restore collection, list, then task explicitly", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 412, height: 920 });
  await open(page, "/app?fixture=trash");
  await page.getByRole("button", { name: "Lists", exact: true }).click();
  await page.getByRole("link", { name: "Trash", exact: true }).click();
  await expect(page.getByText("Restore its collection first.")).toBeVisible();
  await expect(page.getByText("Restore its list first.")).toBeVisible();
  await expect(restore(page, draft)).toHaveCount(0);
  await restore(page, "Spring fair").click();
  await expect(restore(page, "Flyers")).toBeVisible();
  await expect(restore(page, draft)).toHaveCount(0);
  await restore(page, "Flyers").click();
  await restore(page, draft).click();
  await restore(page, paint).click();
  await expect(page.getByText("Nothing in Trash.")).toBeVisible();
  await w.check();
});
