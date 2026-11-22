import { expect, test } from "@playwright/test";
import { open, watch } from "./helpers.mjs";

const attachments = (page) => page.getByRole("region", { name: "Attachments" });
const board = (page) => page.getByRole("button", { name: "Prepare the November board update", exact: true });
const latency = (page) => page.getByRole("button", { name: "Review the API latency report", exact: true });

async function dropOnDetail(page, names) {
  const dataTransfer = await page.evaluateHandle((names) => {
    const data = new DataTransfer();
    for (const name of names) {
      data.items.add(new File(["Example"], name, { type: "text/plain" }));
    }
    return data;
  }, names);
  // Drops work anywhere in the pane, not just on the Add file button.
  const top = page.locator(".detail-top");
  await top.dispatchEvent("dragover", { dataTransfer });
  await top.dispatchEvent("drop", { dataTransfer });
  await dataTransfer.dispose();
}

test("a drop of several files belongs to the task it started on", async ({ page }) => {
  const w = watch(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await open(page);
  const section = attachments(page);
  await latency(page).click();
  await dropOnDetail(page, ["report.txt", "notes.txt"]);
  await expect(section.getByRole("status")).toHaveText("Uploading…");

  await board(page).click();
  await expect(section.getByRole("status")).not.toHaveText("Uploading…");
  await expect(section.locator(".file-row")).toHaveCount(1);
  await latency(page).click();
  await expect(section.locator(".file-name")).toHaveText(["report.txt", "notes.txt"], { timeout: 10_000 });
  await expect(section.getByRole("button", { name: "Add file" })).toBeEnabled();
  await w.check();
});

test("read-only detail does not accept attachments or allow removal", async ({ page }) => {
  const w = watch(page);
  await open(page, "/app?fixture=read-only");
  await board(page).click();
  const section = attachments(page);
  await expect(section.getByRole("button", { name: "Add file" })).toBeDisabled();
  await expect(section.getByRole("button", { name: "Remove “board-pack-draft.pdf”" })).toBeDisabled();
  await dropOnDetail(page, ["notes.txt"]);
  await expect(section.getByRole("status")).not.toHaveText("Uploading…");
  await expect(section.locator(".file-name")).toHaveText(["board-pack-draft.pdf"]);
  await w.check();
});
