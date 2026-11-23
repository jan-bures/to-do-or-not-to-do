import { readFileSync } from "node:fs";
import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";

const fixture = (name) => readFileSync(new URL(`../../../../crates/todo-client/tests/fixtures/eml/${name}.eml`, import.meta.url), "utf8");
const mailFile = (name) => ({ name: `${name}.eml`, text: fixture(name), type: "message/rfc822" });
const title = "Confirm the workshop tomorrow !!!";
const added = (page, name = title) => page.locator(".room").getByRole("button", { name, exact: true });

async function transfer(page, files) {
  return page.evaluateHandle((files) => {
    const data = new DataTransfer();
    for (const { text, name, type } of files) data.items.add(new File([text], name, { type }));
    return data;
  }, files);
}

test("dropping assorted files highlights the room and imports emails into the current list with one Undo", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await open(page);
  await page.locator(".sidebar").getByRole("link", { name: /^Planning/ }).click();
  await expect(page.getByRole("heading", { name: "Planning", exact: true })).toBeVisible();
  const room = page.locator(".room");
  const dataTransfer = await transfer(page, [
    { ...mailFile("plain"), name: "plain.EML", type: "" },
    mailFile("html-only"),
    mailFile("invalid"),
    { ...mailFile("plain"), name: "ordinary.txt", type: "text/plain" },
  ]);
  await room.dispatchEvent("dragenter", { dataTransfer });
  await expect(room).toHaveClass(/email-drop-active/);
  await room.locator(".room-head").dispatchEvent("dragenter", { dataTransfer });
  await room.dispatchEvent("dragleave", { dataTransfer });
  await expect(room).toHaveClass(/email-drop-active/);
  await room.locator(".room-head").dispatchEvent("dragleave", { dataTransfer });
  await expect(room).not.toHaveClass(/email-drop-active/);
  await room.dispatchEvent("dragover", { dataTransfer });
  await room.dispatchEvent("drop", { dataTransfer });
  await dataTransfer.dispose();
  await expect(room).not.toHaveClass(/email-drop-active/);
  const toast = page.locator(".toast");
  await expect(toast).toContainText("Created 2 email tasks · 1 failed");
  await expect(added(page)).toHaveCount(1);
  await added(page).click();
  const detail = page.locator(".detail");
  await expect(detail.getByRole("textbox", { name: "Notes" })).toHaveValue(/Please confirm the workshop agenda/);
  await expect(detail.locator(".source-meta")).toContainText("alice@example.com");
  await expect(detail.locator("a.source")).toHaveCount(0);
  await expect(detail.locator(".file-row")).toHaveCount(0);
  await added(page, "Read the workshop invitation").click();
  const notes = detail.getByRole("textbox", { name: "Notes" });
  await expect(notes).toHaveValue(/Hello & welcome!/);
  await expect(notes).not.toHaveValue(/INVISIBLE|<img/);
  await toast.getByRole("button", { name: "Undo" }).click();
  await expect(added(page)).toHaveCount(0);
  await expect(added(page, "Read the workshop invitation")).toHaveCount(0);
  await w.check();
});

test("smart room uses quick-add's default list; invalid and non-email files don't create tasks", async ({ page }) => {
  const w = watch(page);
  await open(page);
  const room = page.locator(".room");
  const invalid = await transfer(page, [mailFile("invalid")]);
  await room.dispatchEvent("drop", { dataTransfer: invalid });
  await invalid.dispose();
  await expect(page.locator(".toast")).toContainText("Couldn't parse the email file.");
  await expect(page.locator(".toast").getByRole("button", { name: "Undo" })).toHaveCount(0);
  const normal = await transfer(page, [{ ...mailFile("plain"), name: "plain.txt", type: "text/plain" }]);
  await room.dispatchEvent("dragover", { dataTransfer: normal });
  await expect(room).not.toHaveClass(/email-drop-active/);
  await room.dispatchEvent("drop", { dataTransfer: normal });
  await normal.dispose();
  await expect(added(page)).toHaveCount(0);
  const valid = await transfer(page, [mailFile("plain")]);
  await room.dispatchEvent("drop", { dataTransfer: valid });
  await valid.dispose();
  await expect(added(page)).toBeVisible();
  await page.locator(".sidebar").getByRole("link", { name: /^Inbox/ }).click();
  await expect(added(page)).toBeVisible();
  await w.check();
});
