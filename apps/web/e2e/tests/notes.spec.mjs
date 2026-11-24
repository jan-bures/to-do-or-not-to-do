import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";

const viewport = { width: 1440, height: 900 };

async function openNote(page, fixture) {
  await page.setViewportSize(viewport);
  await open(page, `/app?fixture=${fixture}`);
  await page.getByText("Prepare the November board update").first().click();
  return page.locator(".note-preview");
}

test("Markdown notes edit on click or Tab, then render the saved source on blur", async ({ page }) => {
  const problems = watch(page);
  const preview = await openNote(page, "notes-markdown");
  await expect(preview.getByRole("heading", { name: "Workshop plan" })).toBeVisible();
  await expect(preview.locator("strong")).toHaveText("one room");
  await expect(preview.locator("em")).toHaveText("two tables");
  await expect(preview.locator("kbd, img")).toHaveCount(0);
  await expect(preview).toContainText("<kbd>Enter</kbd> and ![map](map.png)");
  const link = preview.getByRole("link", { name: "floor plan" });
  await expect(link).toHaveAttribute("rel", "noopener noreferrer");
  await expect(link).toHaveAttribute("target", "_blank");
  await page.context().route("https://example.com/plan", (route) => route.fulfill({ body: "Floor plan" }));
  const popup = page.waitForEvent("popup");
  await link.click();
  await (await popup).close();
  const editor = page.getByRole("textbox", { name: "Notes", exact: true });
  await expect(editor).toHaveCount(0);
  await preview.getByRole("heading", { name: "Workshop plan" }).click();
  await expect(editor).toBeFocused();
  await expect(editor).toHaveValue(/^# Workshop plan/);
  await editor.fill("## Revised plan\n\n**New** instructions.");
  await editor.press("Tab");
  await expect(preview.getByRole("heading", { name: "Revised plan" })).toBeVisible();
  await expect(preview.locator("strong")).toHaveText("New");
  await page.keyboard.press("Shift+Tab");
  await expect(editor).toBeFocused();
  await expect(editor).toHaveValue("## Revised plan\n\n**New** instructions.");
  await problems.check();
});

test("checking a Markdown item saves only that marker and offers Undo without editing", async ({ page }) => {
  const problems = watch(page);
  const preview = await openNote(page, "notes-markdown");
  const projector = preview.getByRole("checkbox", { name: "Test the projector" });
  const room = preview.getByRole("checkbox", { name: "Confirm the room" });
  await expect(projector).not.toBeChecked();
  await expect(room).toBeChecked();
  await projector.click();
  await expect(projector).toBeChecked();
  await expect(room).toBeChecked();
  await expect(page.locator("textarea.notes")).toHaveCount(0);
  await page.locator(".toast").getByRole("button", { name: "Undo" }).click();
  await expect(projector).not.toBeChecked();
  await room.press("Space");
  await expect(room).not.toBeChecked();
  await expect(room).toBeFocused();
  await preview.getByRole("heading", { name: "Workshop plan" }).click();
  await expect(page.locator("textarea.notes")).toHaveValue(/- \[ \] Test the projector\n- \[ \] Confirm the room/);
  await problems.check();
});

for (const colorScheme of ["light", "dark"]) {
  test.describe(`notes gallery in ${colorScheme}`, () => {
    test.use({ colorScheme, deviceScaleFactor: 2 });
    for (const fixture of ["notes-markdown", "notes-code"]) {
      test(`${fixture} at 200%`, async ({ page }) => {
        const problems = watch(page);
        const preview = await openNote(page, fixture);
        await expect(preview.locator("pre").first()).toBeVisible();
        expect(await preview.evaluate((note) => note.scrollWidth)).toBe(await preview.evaluate((note) => note.clientWidth));
        await preview.screenshot({ path: `gallery/${fixture}-${colorScheme}-200.png`, animations: "disabled" });
        await problems.check();
      });
    }
  });
}
