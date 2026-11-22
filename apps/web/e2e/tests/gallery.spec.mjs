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

    test(`editing due date and steps (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page);
      await page.getByRole("button", { name: "Prepare the November board update", exact: true }).click();
      await page.keyboard.press("d");
      const due = page.getByRole("textbox", { name: "Due date" });
      await due.fill("tomorrow");
      await expect(due).toBeFocused();
      await shot(page, `desktop-due-editor-${scheme}`);
      await due.press("Escape");
      await page.locator(".step-title").nth(2).hover();
      await expect(page.locator(".step-actions").nth(2)).toHaveCSS("opacity", "1");
      await shot(page, `desktop-step-actions-${scheme}`);
      await page.getByRole("button", { name: "Delete step 3" }).click();
      await expect(page.locator(".toast").getByRole("button", { name: /Undo/ })).toBeVisible();
      await expect(page.getByRole("textbox", { name: "Add step" })).toBeFocused();
      await shot(page, `desktop-step-undo-${scheme}`);
      await w.check();
    });

    test(`quick-add previews the shared date label (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page);
      await page.getByRole("textbox", { name: "Add a task" }).fill("Water the plants zítra");
      await expect(page.locator(".qa-date-chip")).toHaveText("Sun 22 Nov");
      await shot(page, `desktop-quick-add-date-${scheme}`);
      await page.setViewportSize(PHONE);
      await shot(page, `phone-quick-add-date-${scheme}`);
      await w.check();
    });

    test(`desktop collection (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page);
      await page.locator(".sidebar").getByRole("link", { name: /^Work/ }).click();
      await page.locator(".room-title", { hasText: "Work" }).waitFor();
      await page.getByRole("button", { name: "Collapse Work" }).click();
      await shot(page, `desktop-collection-${scheme}`);
      await w.check();
    });

    test(`finished day (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page, "/app?fixture=finished");
      await page.locator(".room-head.finished").waitFor();
      await shot(page, `finished-${scheme}`);
      await page.getByRole("button", { name: "Připravit nedělní oběd" }).click();
      await expect(page.locator(".detail .d-title")).toHaveText("Připravit nedělní oběd");
      await shot(page, `finished-selected-${scheme}`);
      await page.getByRole("button", { name: "Close", exact: true }).click();
      await page.getByRole("button", { name: "Completed 12" }).click();
      await expect(page.locator(".completed-rows .row")).toHaveCount(12);
      await shot(page, `finished-completed-${scheme}`);
      await w.check();
    });

    test(`finished day with nothing tomorrow on phone (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(PHONE);
      await open(page, "/app?fixture=finished-no-tomorrow");
      await expect(page.getByRole("link", { name: "Plan Sunday" })).toBeVisible();
      await shot(page, `phone-finished-no-tomorrow-${scheme}`);
      await w.check();
    });

    test(`desktop query filter (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page);
      await page.keyboard.press("Control+k");
      await page.getByRole("searchbox").fill("tag:docs");
      await expect(page.locator(".row")).toHaveCount(1);
      await shot(page, `desktop-filter-${scheme}`);
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

    test(`storage error (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page, "/app?fixture=storage-error");
      await page.locator(".toast-error[role=alert]").waitFor();
      await shot(page, `storage-error-${scheme}`);
      await page.setViewportSize(PHONE);
      await page.locator(".toast-error[role=alert]").waitFor();
      await shot(page, `phone-storage-error-${scheme}`);
      await w.check();
    });

    test(`read only after an upgrade elsewhere (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page, "/app?fixture=read-only");
      await page.locator(".toast-read-only[role=alert]").waitFor();
      await shot(page, `read-only-${scheme}`);
      await page.setViewportSize(PHONE);
      await page.locator(".toast-read-only[role=alert]").waitFor();
      await shot(page, `phone-read-only-${scheme}`);
      await w.check();
    });

    test(`medium window with detail (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(MEDIUM);
      await open(page);
      await page.getByText("Review the API latency report").first().click();
      await page.locator(".detail .d-title").waitFor();
      await expect(page.locator(".scrim")).toHaveCSS("opacity", "1");
      await shot(page, `medium-${scheme}`);
      const detail = page.getByRole("dialog", { name: "Task detail" });
      await detail.getByRole("checkbox", { name: "Complete", exact: true }).click();
      await expect(detail.getByRole("button", { name: /Undo/ })).toBeVisible();
      await shot(page, `medium-undo-${scheme}`);
      await w.check();
    });

    test(`phone my day (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(PHONE);
      await open(page);
      await expect(page.locator(".row")).toHaveCount(9);
      await shot(page, `phone-${scheme}`);
      await page.getByText("Prepare the November board update").first().click();
      await page.locator(".detail .d-title").waitFor();
      await shot(page, `phone-detail-${scheme}`);
      await page.getByRole("button", { name: "Close", exact: true }).click();
      await page.getByRole("button", { name: "Lists", exact: true }).click();
      await expect(page.getByRole("dialog", { name: "Lists" })).toBeVisible();
      await shot(page, `phone-drawer-${scheme}`);
      await w.check();
    });
  });
}
