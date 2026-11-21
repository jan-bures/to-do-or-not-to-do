// The screenshot gallery (docs/design-language.md, "The screenshot gate"): every screen in both themes,
// from invented fixture data with a fixed clock (Saturday 21 November 2026, 10:35). The PNGs land in
// e2e/gallery/ (CI uploads them); compare them with the approved mockups.
import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";

const DESKTOP = { width: 1440, height: 900 };
const MEDIUM = { width: 1024, height: 768 };
const PHONE = { width: 412, height: 920 };

const shot = (page, name) => page.screenshot({ path: `gallery/${name}.png`, animations: "disabled" });

for (const scheme of ["light", "dark"]) {
  test.describe(`${scheme}`, () => {
    test.use({ colorScheme: scheme });

    test(`desktop my day with detail (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page);
      await page.getByText("Prepare the November board update").first().click();
      await page.locator(".detail .d-title").waitFor();
      await shot(page, `desktop-${scheme}`);
      await w.check();
    });

    test(`desktop collection (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page);
      await page.locator(".sidebar").getByRole("link", { name: /^Work/ }).click();
      await page.locator(".room-title", { hasText: "Work" }).waitFor();
      await shot(page, `desktop-collection-${scheme}`);
      await w.check();
    });

    test(`finished day (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page, "/app?fixture=finished");
      await page.locator(".room-head.finished").waitFor();
      await page.waitForTimeout(1300);
      await shot(page, `finished-${scheme}`);
      await w.check();
    });

    test(`empty list (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page);
      await page.locator(".sidebar").getByRole("link", { name: /^Garden/ }).click();
      await page.locator(".empty-line").waitFor();
      await shot(page, `empty-list-${scheme}`);
      await w.check();
    });

    test(`medium window with detail (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(MEDIUM);
      await open(page);
      await page.getByText("Review the API latency report").first().click();
      await page.locator(".detail .d-title").waitFor();
      await page.waitForTimeout(250);
      await shot(page, `medium-${scheme}`);
      await w.check();
    });

    test(`phone my day (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(PHONE);
      await open(page);
      // Without a date the chips start flush with the title; with one, the date comes first.
      const left = (row, sel) => row.locator(sel).first().evaluate((e) => e.getBoundingClientRect().left);
      const undated = page.locator(".row").filter({ hasText: "Review the API latency report" });
      expect(await undated.locator(".due-slot").count()).toBe(0);
      expect(Math.abs((await left(undated, ".chip")) - (await left(undated, ".row-title")))).toBeLessThan(1);
      const dated = page.locator(".row").filter({ hasText: "Prepare the November board update" });
      expect(Math.abs((await left(dated, ".due-slot")) - (await left(dated, ".row-title")))).toBeLessThan(1);
      expect(await left(dated, ".chip")).toBeGreaterThan(await left(dated, ".due-slot"));
      await shot(page, `phone-${scheme}`);
      await page.getByText("Prepare the November board update").first().click();
      await page.locator(".detail .d-title").waitFor();
      await shot(page, `phone-detail-${scheme}`);
      await page.locator(".detail .back").click();
      await page.locator(".app-bar .icon-button").click();
      await page.waitForTimeout(250);
      await shot(page, `phone-drawer-${scheme}`);
      await w.check();
    });
  });
}
