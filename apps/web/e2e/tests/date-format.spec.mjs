import { test, expect } from "@playwright/test";

async function openFormat(page, theme) {
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const item = page.getByRole("menuitem", { name: /^Date format/ });
  if (theme) {
    await expect(item).toContainText("21.11.");
    await page.getByRole("menu", { name: "Settings" }).screenshot({
      path: `gallery/date-format-row-${theme}.png`, animations: "disabled",
    });
  }
  await item.click();
  return page.getByRole("dialog", { name: "Date format" });
}

test("date format is applied immediately and remembered on reload", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto("/app/");
  const eyebrow = page.locator("main .eyebrow").first();
  await expect(eyebrow).toContainText("21.11.");
  const dialog = await openFormat(page);
  const select = dialog.getByRole("combobox", { name: "Date format" });
  await expect(select.locator("option")).toHaveText([
    "Day.Month — 21.11.", "Day Month name — 21 Nov", "Month name Day (US) — Nov 21",
    "ISO — 2026-11-21", "Numeric US — 11/21",
  ]);
  await expect(select).toHaveValue("day_month");
  await select.selectOption("iso");
  await dialog.getByRole("button", { name: "Done" }).click();
  await expect(eyebrow).toContainText("2026-11-21");
  await page.reload();
  await expect(eyebrow).toContainText("2026-11-21");
  await expect((await openFormat(page)).getByRole("combobox")).toHaveValue("iso");
});

for (const theme of ["light", "dark"]) {
  test(`date format gallery ${theme}`, async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
    await page.goto("/app/");
    const dialog = await openFormat(page, theme);
    await expect(dialog.getByRole("combobox")).toHaveValue("day_month");
    await page.screenshot({ path: `gallery/date-format-${theme}.png`, animations: "disabled" });
  });
}
