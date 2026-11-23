import { readFileSync } from "node:fs";
import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";

const fixture = (name) => readFileSync(new URL(`../../../../crates/todo-client/tests/fixtures/eml/${name}.eml`, import.meta.url), "utf8");
const mailFile = (name) => ({ name: `${name}.eml`, text: fixture(name), type: "message/rfc822" });
const title = "Confirm the workshop tomorrow !!!";
const list = (page) => page.locator(".task-list-area");
const feedback = (page) => page.locator(".email-drop-feedback");
const added = (page, name = title) => list(page).getByRole("button", { name, exact: true });

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
  const target = list(page);
  const card = feedback(page);
  const dataTransfer = await transfer(page, [
    { ...mailFile("plain"), name: "plain.EML", type: "" },
    mailFile("html-only"),
    mailFile("invalid"),
    { ...mailFile("plain"), name: "ordinary.txt", type: "text/plain" },
  ]);
  for (const outside of [page.locator(".room-head"), page.locator(".quick-add")]) {
    await outside.dispatchEvent("dragenter", { dataTransfer });
    await outside.dispatchEvent("dragover", { dataTransfer });
    await outside.dispatchEvent("drop", { dataTransfer });
    await expect(card).not.toHaveClass(/\bactive\b/);
    await expect(added(page)).toHaveCount(0);
  }
  await target.dispatchEvent("dragenter", { dataTransfer });
  await expect(card).toHaveCSS("opacity", "1");
  await expect(card.getByText("Drop .eml files to create tasks")).toBeVisible();
  await expect(card.getByText("into Planning")).toBeVisible();
  await expect(page.locator(".email-drop-announcement")).toHaveText("Drop .eml files to create tasks into Planning");
  await expect(page.locator(".toast-announcement")).toBeEmpty();
  expect((await card.boundingBox()).y).toBeCloseTo((await target.boundingBox()).y, 0);
  const row = target.locator(".row").first();
  await row.dispatchEvent("dragenter", { dataTransfer });
  await target.dispatchEvent("dragleave", { dataTransfer });
  await expect(card).toHaveClass(/\bactive\b/);
  await row.dispatchEvent("dragleave", { dataTransfer });
  await expect(card).toHaveCSS("opacity", "0");
  await target.dispatchEvent("dragover", { dataTransfer });
  await target.dispatchEvent("drop", { dataTransfer });
  await dataTransfer.dispose();
  await expect(card).not.toHaveClass(/\bactive\b/);
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

test("the affordance follows the visible part of a scrolled list without shifting rows", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 1280, height: 500 });
  await open(page);
  const room = page.locator(".room");
  const target = list(page);
  const overlay = feedback(page);
  const transferData = await transfer(page, [mailFile("plain")]);
  await target.dispatchEvent("dragenter", { dataTransfer: transferData });
  await expect(overlay).toHaveCSS("opacity", "1");
  const scroll = await room.evaluate((el) => {
    el.scrollTop = el.scrollHeight;
    return el.scrollTop;
  });
  expect(scroll).toBeGreaterThan(0);
  const viewport = await room.boundingBox();
  await expect.poll(async () => (await overlay.boundingBox()).y).toBeCloseTo(viewport.y, 0);
  expect((await overlay.boundingBox()).height).toBeCloseTo(viewport.height, 0);
  const card = await overlay.locator(".email-drop-card").boundingBox();
  expect(card.y + card.height / 2).toBeCloseTo(viewport.y + viewport.height / 2, 0);
  await target.dispatchEvent("dragleave", { dataTransfer: transferData });
  await transferData.dispose();
  await expect(overlay).toHaveCSS("opacity", "0");
  expect(await room.evaluate((el) => el.scrollTop)).toBe(scroll);
  await w.check();
});

test("smart room uses quick-add's default list; invalid and non-email files don't create tasks", async ({ page }) => {
  const w = watch(page);
  await open(page);
  const room = list(page);
  const card = feedback(page);
  const invalid = await transfer(page, [mailFile("invalid")]);
  await room.dispatchEvent("drop", { dataTransfer: invalid });
  await invalid.dispose();
  await expect(page.locator(".toast")).toContainText("Couldn't parse the email file.");
  await expect(page.locator(".toast").getByRole("button", { name: "Undo" })).toHaveCount(0);
  const normal = await transfer(page, [{ ...mailFile("plain"), name: "plain.txt", type: "text/plain" }]);
  await room.dispatchEvent("dragover", { dataTransfer: normal });
  await expect(card).not.toHaveClass(/\bactive\b/);
  await room.dispatchEvent("drop", { dataTransfer: normal });
  await normal.dispose();
  await expect(added(page)).toHaveCount(0);
  const valid = await transfer(page, [mailFile("plain")]);
  await room.dispatchEvent("dragover", { dataTransfer: valid });
  await expect(card).toHaveCSS("opacity", "1");
  await expect(card.getByText("into Inbox")).toBeVisible();
  await room.dispatchEvent("drop", { dataTransfer: valid });
  await valid.dispose();
  await expect(added(page)).toBeVisible();
  await page.locator(".sidebar").getByRole("link", { name: /^Inbox/ }).click();
  await expect(added(page)).toBeVisible();
  await w.check();
});
