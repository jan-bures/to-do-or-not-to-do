// The screenshot gallery (docs/design-language.md, "The screenshot gate"): every screen in both themes,
// from invented fixture data with a fixed clock (Saturday 21 November 2026, 10:35). The PNGs land in
// e2e/gallery/ (CI uploads them); compare them with the approved mockups.
import { expect, test } from "@playwright/test";
import { chooseNew, dragOver, open, openDeviceSettings, openRows, pickup, watch } from "./helpers.mjs";
import { beforeReminder, mockNotifications, reminderToast, REMINDER_TASK } from "./reminder-helpers.mjs";
import { mockPush } from "./web-push-helpers.mjs";

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
      const chip = page.locator(".row .chip").first();
      await expect(chip.locator(":scope > *")).toHaveCount(1);
      await expect(chip).toHaveCSS("padding-inline-start", "8px");
      await expect(chip).toHaveCSS("padding-inline-end", "8px");
      await expect(page.locator(".crumb .chip > *")).toHaveClass("chip-label");
      await expect(page.locator(".tag-trigger .tag-label").first()).toHaveText(/^#/);
      const overdue = page.locator('.col-due [data-tone="overdue"]').first();
      await expect(overdue).toBeVisible();
      await expect(overdue.locator(":scope > *")).toHaveCount(0);
      const sidebar = page.locator(".sidebar");
      await expect(sidebar.locator(".nav-pip")).toHaveText(/^\d+ new$/);
      await expect(sidebar.locator(".nav-pip > *")).toHaveCount(0);
      await shot(page, `desktop-${scheme}`);
      await sidebar.screenshot({ path: `gallery/sidebar-${scheme}.png`, animations: "disabled" });
      await sidebar.getByRole("button", { name: "New", exact: true }).press("Enter");
      await expect(sidebar.getByRole("menuitem", { name: "List", exact: true })).toBeFocused();
      await sidebar.screenshot({ path: `gallery/sidebar-new-menu-${scheme}.png`, animations: "disabled" });
      await w.check();
    });

    for (const [fixture, connection, label] of [
      ["", "online", /^\d{2}:\d{2}$/],
      ["syncing", "syncing", "Syncing…"],
      ["offline", "offline", "Offline"],
      ["not-synced", "error", "2 not synced"],
    ]) {
      test(`sync footer ${connection} (${scheme})`, async ({ page }) => {
        await page.setViewportSize(DESKTOP);
        await open(page, fixture ? `/app?fixture=${fixture}` : "/app");
        const footer = page.locator(".sidebar .side-foot");
        await expect(footer.locator(".conn")).toHaveAttribute("data-conn", connection);
        await expect(footer.locator(".sync-text")).toHaveText(label);
        await footer.screenshot({ path: `gallery/footer-${connection}-${scheme}.png`, animations: "disabled" });
      });
    }

    for (const [permission, name, status] of [
      ["default", "not-enabled", "Not enabled"],
      ["granted", "allowed", "Allowed"],
      ["denied", "blocked", "Blocked"],
      ["unsupported", "unsupported", "Not supported"],
    ]) {
      test(`reminder notifications ${name} in Settings (${scheme})`, async ({ page }) => {
        const w = watch(page);
        await mockNotifications(page, permission);
        await page.setViewportSize(DESKTOP);
        await open(page);
        await page.getByRole("button", { name: "Settings", exact: true }).click();
        await expect(page.locator("#reminder-notification-description")).toContainText(status);
        await page.locator(".sidebar").screenshot({ path: `gallery/reminder-settings-${name}-${scheme}.png`, animations: "disabled" });
        await w.check();
      });
    }

    for (const [name, permission, status] of [
      ["off", "default", "Off."], ["on", "granted", "On."], ["blocked", "denied", "Blocked by the browser"],
    ]) {
      test(`background reminders ${name} in Settings (${scheme})`, async ({ page, browser, baseURL }) => {
        const capture = async (tab, suffix, crop = false) => {
          const w = watch(tab);
          await mockPush(tab, permission, name === "on");
          await open(tab);
          await tab.getByRole("button", { name: "Settings", exact: true }).click();
          await expect(tab.locator("#reminder-notification-description")).toContainText(status);
          const sidebar = tab.locator(".sidebar");
          await expect(sidebar.getByRole("menuitem", { name: "Send test notification" })).toHaveCount(name === "on" ? 1 : 0);
          const gap = await tab.locator("#reminder-notification-description").evaluate(p => {
            const label = document.querySelector("#reminder-notification-setting > span:not(.icon)");
            return p.getBoundingClientRect().left + parseFloat(getComputedStyle(p).paddingLeft)
              - label.getBoundingClientRect().left;
          });
          expect(Math.abs(gap)).toBeLessThan(0.5);
          const area = crop ? sidebar.locator(".settings-menu") : sidebar;
          await area.screenshot({ path: `gallery/web-push-${name}-${scheme}${suffix}.png`, animations: "disabled" });
          await w.check();
        };
        await page.setViewportSize(DESKTOP);
        await capture(page, "");
        const scaled = await browser.newPage({ baseURL, viewport: DESKTOP, colorScheme: scheme,
          deviceScaleFactor: 1.5, locale: "en-GB", timezoneId: "Europe/Prague" });
        try { await capture(scaled, "-150"); } finally { await scaled.close(); }
        if (name !== "off") {
          const cropped = await browser.newPage({ baseURL, viewport: DESKTOP, colorScheme: scheme,
            deviceScaleFactor: 2.5, locale: "en-GB", timezoneId: "Europe/Prague" });
          try { await capture(cropped, "-menu-250", true); } finally { await cropped.close(); }
        }
      });
    }

    test(`reminder fallback toast (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await mockNotifications(page, "denied");
      await page.setViewportSize(DESKTOP);
      await beforeReminder(page);
      await page.clock.runFor(10000);
      await expect(reminderToast(page)).toContainText(REMINDER_TASK);
      await shot(page, `reminder-toast-${scheme}`);
      await page.setViewportSize(PHONE);
      await page.clock.runFor(32);
      await shot(page, `reminder-toast-phone-${scheme}`);
      await w.check();
    });

    test(`email drop target (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page);
      await page.getByText("Prepare the November board update").first().click();
      await page.locator(".detail .d-title").waitFor();
      const dataTransfer = await page.evaluateHandle(() => {
        const data = new DataTransfer();
        data.items.add(new File([], "workshop.eml", { type: "message/rfc822" }));
        return data;
      });
      const target = page.locator(".task-list-area");
      await target.dispatchEvent("dragenter", { dataTransfer });
      const feedback = page.locator(".email-drop-feedback");
      await expect(feedback).toHaveCSS("opacity", "1");
      await expect(feedback.getByText("into Inbox")).toBeVisible();
      await shot(page, `email-drop-${scheme}`);
      await target.dispatchEvent("dragleave", { dataTransfer });
      await dataTransfer.dispose();
      await w.check();
    });

    test(`task drag pickup and target (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page);
      const sidebar = page.locator(".sidebar");
      await sidebar.getByRole("link", { name: /^Team/ }).click();
      const rows = openRows(page);
      await rows.first().locator(".row-title").click({ modifiers: ["Control"] });
      await rows.nth(1).locator(".row-title").click({ modifiers: ["Shift"] });
      await pickup(page, rows.first());
      await dragOver(page, rows.last(), 0.8);
      await expect(rows.last().locator(".task-insertion-line")).toBeVisible();
      await shot(page, `drag-reorder-${scheme}`);
      const destination = sidebar.getByRole("link", { name: /^Planning/ });
      await dragOver(page, destination);
      await expect(destination).toHaveClass(/task-drop-target/);
      await shot(page, `drag-destination-${scheme}`);
      await page.keyboard.press("Escape");
      await page.mouse.up();
      await w.check();
    });

    test(`task multi-selection (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page);
      const rows = page.locator(".rows:not(.completed-rows) .row-title");
      await rows.first().click({ modifiers: ["Control"] });
      await rows.nth(2).click({ modifiers: ["Shift"] });
      await expect(rows.nth(2)).toBeFocused();
      await page.keyboard.press("Shift+ArrowUp");
      await expect(rows.nth(1)).toBeFocused();
      await page.keyboard.press("Shift+ArrowDown");
      await expect(rows.nth(2)).toBeFocused();
      await expect(page.locator(".bulk-count")).toHaveText("3 tasks selected");
      await page.mouse.move(0, DESKTOP.height - 1);
      const selected = page.locator(".row.bulk-selected");
      await expect(selected).toHaveCount(3);
      await expect.poll(() => selected.evaluateAll((items) => {
        // Membership is visible on all three rows, including the two without keyboard focus.
        const room = items[0].closest(".main");
        const hex = getComputedStyle(room).getPropertyValue("--color-selection-wash").trim();
        const channels = hex.match(/[0-9a-f]{2}/gi)?.map((pair) => parseInt(pair, 16));
        const wash = channels && `rgb(${channels.join(", ")})`;
        return items.map((item) => ({
          wash: getComputedStyle(item).backgroundColor === wash,
          bar: getComputedStyle(item, "::before").width === getComputedStyle(item).getPropertyValue("--selection-bar").trim(),
          cursor: getComputedStyle(item, "::after").borderWidth !== "0px",
        }));
      })).toEqual([
        { wash: true, bar: true, cursor: false },
        { wash: true, bar: true, cursor: false },
        { wash: true, bar: true, cursor: true },
      ]);
      await shot(page, `multi-select-${scheme}`);
      await page.setViewportSize(PHONE);
      await expect(page.locator(".bulk-count")).toBeVisible();
      await shot(page, `multi-select-phone-${scheme}`);
      await w.check();
    });

    test(`task activity on desktop and phone (${scheme})`, async ({ page }) => {
      const w = watch(page);
      for (const [layout, viewport] of [["desktop", DESKTOP], ["phone", PHONE]]) {
        await page.setViewportSize(viewport);
        await open(page, "/app/completed?fixture=task-activity");
        await page.getByRole("button", { name: "Review the color proofs", exact: true }).click();
        const summary = page.getByRole("region", { name: "Activity", exact: true });
        await expect(summary.locator(".act-text")).toHaveText([
          "You edited this", "Pi completed this", "Codex created this",
        ]);
        await expect(summary.locator(".act-when")).toHaveText(["12 min ago", "1 hr ago", "2 days ago"]);
        await summary.scrollIntoViewIfNeeded();
        await shot(page, `task-activity-${layout}-${scheme}`);
      }
      await w.check();
    });

    test(`trash populated and empty (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page, "/app/trash?fixture=trash");
      await expect(page.getByRole("button", { name: "Restore Spring fair", exact: true })).toBeVisible();
      await shot(page, `trash-${scheme}`);
      await page.setViewportSize(PHONE);
      await shot(page, `trash-phone-${scheme}`);
      await open(page, "/app/trash");
      await expect(page.getByText("Nothing in Trash.")).toBeVisible();
      await shot(page, `trash-empty-phone-${scheme}`);
      await page.setViewportSize(DESKTOP);
      await shot(page, `trash-empty-${scheme}`);
      await w.check();
    });

    for (const [fixture, viewport] of [
      ["detail-wide", DESKTOP],
      ["detail-wide-1920", { width: 1920, height: 1080 }],
      ["detail-narrow", DESKTOP],
      ["resize-handle-hover", DESKTOP],
    ]) {
      test(`${fixture} (${scheme})`, async ({ page }) => {
        const w = watch(page);
        const wide = fixture.startsWith("detail-wide");
        await page.setViewportSize(viewport);
        await open(page, wide ? "/app?fixture=detail-wide" : "/app");
        const title = wide ? /^Prepare the quarterly roadmap review/ : "Prepare the November board update";
        await page.getByRole("button", { name: title, exact: !wide }).click();
        const handle = page.getByRole("separator", { name: "Task detail width" });
        if (wide) {
          await handle.press("End");
          await expect(page.locator(".app")).toHaveClass(/\bcompact-room\b/);
        } else if (fixture === "detail-narrow") {
          for (let i = 0; i < 3; i++) await handle.press("Shift+ArrowRight");
        }
        if (fixture === "resize-handle-hover") {
          await handle.hover();
          await expect.poll(() => handle.evaluate((el) => getComputedStyle(el, "::after").opacity)).toBe("1");
        } else {
          // Capture the pane at rest, without keyboard focus or pointer hover.
          await handle.evaluate((el) => el.blur());
          await page.mouse.move(0, viewport.height - 1);
        }
        await shot(page, `${fixture}-${scheme}`);
        await w.check();
      });
    }

    test(`narrow sidebar with four active agents (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize({ width: 240, height: 900 });
      await open(page, "/app?fixture=agent-rows");
      await page.getByRole("button", { name: "Lists", exact: true }).click();
      const sidebar = page.getByRole("dialog", { name: "Lists" });
      await expect(sidebar.getByRole("region", { name: "Agents" }).getByRole("link")).toHaveCount(4);
      await sidebar.screenshot({ path: `gallery/sidebar-narrow-${scheme}.png`, animations: "disabled" });
      await w.check();
    });

    test(`resting toggle states (${scheme})`, async ({ page }) => {
      await page.setViewportSize(DESKTOP);
      await open(page, "/app/all");
      await page.getByRole("button", { name: "Prepare the November board update", exact: true }).click();
      const detail = page.locator(".detail");
      const sun = detail.getByRole("button", { name: "My Day" });
      const star = detail.getByRole("button", { name: "Important" });
      await expect(sun).toHaveAttribute("aria-pressed", "true");
      await star.click();
      await expect(star).toHaveAttribute("aria-pressed", "true");
      await page.mouse.move(0, 0);
      await shot(page, `toggle-states-on-${scheme}`);
      await star.click();
      await sun.click();
      await expect(sun).toHaveAttribute("aria-pressed", "false");
      await expect(star).toHaveAttribute("aria-pressed", "false");
      await page.mouse.move(0, 0);
      await shot(page, `toggle-states-off-${scheme}`);
    });

    test(`settings menu on desktop and phone (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page);
      const settings = page.getByRole("button", { name: "Settings", exact: true });
      const admin = page.getByRole("menuitem", { name: "Open admin pages", exact: true });
      await settings.press("Enter");
      await expect(admin).toBeFocused();
      await page.locator(".sidebar").screenshot({ path: `gallery/sidebar-settings-${scheme}.png`, animations: "disabled" });
      await page.keyboard.press("Escape");
      await page.setViewportSize(PHONE);
      await page.getByRole("button", { name: "Lists", exact: true }).click();
      await settings.press("Enter");
      await expect(admin).toBeFocused();
      await shot(page, `phone-settings-${scheme}`);
      await w.check();
    });

    test(`device invite and approval settings (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page);
      await openDeviceSettings(page);
      await page.getByRole("button", { name: "Pair a new device" }).click();
      await page.getByRole("button", { name: "Create invite" }).click();
      await expect(page.getByRole("img", { name: "Invite QR code" })).toBeVisible();
      await expect(page.getByText("ABCDEF-GHJKMN")).toBeVisible();
      await shot(page, `device-invite-${scheme}`);
      await page.getByRole("button", { name: "Cancel invite" }).click();
      await expect(page.getByText("Invite canceled.")).toBeVisible();
      await page.getByRole("button", { name: "Back", exact: true }).click();
      await page.getByRole("button", { name: "Approve a device" }).click();
      await page.getByRole("textbox", { name: "Code shown on the device" }).fill("abcd efgh");
      await page.getByRole("dialog", { name: "Approve a device" }).getByRole("button", { name: "Review", exact: true }).click();
      await expect(page.getByText(/Approve “New phone”/)).toBeVisible();
      await expect(page.getByText("ABCD-EFGH", { exact: true })).toBeVisible();
      await expect(page.getByText("Source address: 203.0.113.7")).toBeVisible();
      await shot(page, `device-approve-${scheme}`);
      await page.getByRole("button", { name: "Use another code" }).click();
      await expect(page.getByRole("textbox", { name: "Code shown on the device" })).toBeFocused();
      await w.check();
    });

    test(`detail due calendar (${scheme})`, async ({ page }) => {
      const w = watch(page);
      for (const [layout, viewport] of [["desktop", DESKTOP], ["phone", PHONE]]) {
        await page.setViewportSize(viewport);
        await open(page);
        await page.getByRole("button", { name: "Prepare the November board update", exact: true }).click();
        const due = page.getByRole("button", { name: /^Edit due date:/ });
        await due.click();
        const calendar = page.getByRole("dialog", { name: "Choose due date" });
        await expect(calendar.getByRole("button", { name: "Saturday, 21 November 2026" })).toBeFocused();
        await expect(calendar.getByText("This task repeats. No date also stops the repeat.")).toBeInViewport();
        await shot(page, `${layout}-detail-due-calendar-${scheme}`);
        await page.keyboard.press("Escape");
        await expect(due).toBeFocused();
      }
      await w.check();
    });

    test(`editing steps (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page);
      await page.getByRole("button", { name: "Prepare the November board update", exact: true }).click();
      await page.locator(".step-title").nth(2).hover();
      await expect(page.locator(".step-actions").nth(2)).toHaveCSS("opacity", "1");
      await shot(page, `desktop-step-actions-${scheme}`);
      await page.getByRole("button", { name: "Delete step 3" }).click();
      await expect(page.locator(".toast").getByRole("button", { name: /Undo/ })).toBeVisible();
      await expect(page.getByRole("textbox", { name: "Add step" })).toBeFocused();
      await shot(page, `desktop-step-undo-${scheme}`);
      await w.check();
    });

    test(`attachments: empty, uploading, refused and removed with Undo (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page, "/app?fixture=upload-fails");
      const section = page.getByRole("region", { name: "Attachments" });
      const add = section.getByRole("button", { name: "Add file" });
      await page.getByRole("button", { name: "Review the API latency report", exact: true }).click();
      await expect(add).toBeEnabled();
      await expect(section.locator(".file-row")).toHaveCount(0);
      await section.scrollIntoViewIfNeeded();
      await shot(page, `attachments-empty-${scheme}`);

      // Freeze the fixture's simulated network delay to capture a real, in-flight selection.
      await page.clock.install({ time: new Date("2026-11-21T10:35:00Z") });
      await page.clock.pauseAt(new Date("2026-11-21T10:36:00Z"));
      const picker = page.waitForEvent("filechooser");
      await add.click();
      await (await picker).setFiles({ name: "notes.txt", mimeType: "text/plain", buffer: Buffer.from("Notes") });
      await expect(section.getByRole("status")).toHaveText("Uploading…");
      await expect(add).toBeDisabled();
      await shot(page, `attachments-uploading-${scheme}`);
      await page.clock.runFor(2000);
      await expect(section.getByRole("alert")).toHaveText("Couldn’t upload the selected files.");
      await expect(add).toBeEnabled();
      await expect(section.locator(".file-row")).toHaveCount(0);
      await shot(page, `attachments-failed-${scheme}`);
      await page.clock.resume();

      await page.locator(".detail").getByRole("button", { name: "Close", exact: true }).click();
      await expect(section).toBeHidden();
      await page.getByRole("button", { name: "Review the API latency report", exact: true }).click();
      await expect(add).toBeEnabled();
      await expect(section.getByRole("alert")).toHaveCount(0);

      await page.getByRole("button", { name: "Prepare the November board update", exact: true }).click();
      const remove = section.getByRole("button", { name: "Remove “board-pack-draft.pdf”" });
      await expect(remove).toBeVisible();
      await section.scrollIntoViewIfNeeded();
      await shot(page, `attachments-populated-${scheme}`);
      await remove.click();
      await expect(remove).toHaveCount(0);
      await expect(add).toBeFocused();
      await expect(page.locator(".toast")).toContainText("Removed attachment “board-pack-draft.pdf”");
      const undo = page.getByRole("button", { name: /^Undo/ });
      await expect(undo).toBeVisible();
      await shot(page, `attachments-removed-undo-${scheme}`);
      await undo.click();
      await expect(remove).toBeVisible();
      await w.check();
    });

    test(`Activity history, filter, empty log and refused undo (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page, "/app/activity");
      await expect(page.getByRole("heading", { name: "Today", exact: true })).toBeVisible();
      await shot(page, `activity-${scheme}`);

      const undo = page.getByRole("button", { name: /^Undo: Updated task.*due date/ });
      await undo.click();
      await expect(page.locator(".activity-refusal").filter({
        hasText: "Couldn't undo: the item was changed again.",
      })).toBeVisible();
      await shot(page, `activity-refused-${scheme}`);

      await page.getByRole("region", { name: "Agents" }).getByRole("link", { name: /Codex/ }).click();
      await expect(page).toHaveURL(/\/app\/activity\/agent\/codex$/);
      await expect(page.locator(".activity-row").first()).toContainText("Codex");
      await shot(page, `activity-agent-${scheme}`);

      await open(page, "/app/activity?fixture=blank");
      await expect(page.getByText("No activity yet.")).toBeVisible();
      await shot(page, `activity-empty-${scheme}`);
      await w.check();
    });

    test(`quick-add chips and pickers (${scheme})`, async ({ page }) => {
      const w = watch(page);
      for (const [layout, size] of [["desktop", DESKTOP], ["phone", PHONE]]) {
        await page.setViewportSize(size);
        await open(page);
        await page.getByRole("textbox", { name: "Add a task" }).fill("Water the plants zítra !!");
        await expect(page.locator(".qa-date-chip")).toHaveText("Sun 22.11.");
        await shot(page, `${layout}-quick-add-chips-${scheme}`);
        await page.getByRole("button", { name: "Choose due date" }).click();
        await expect(page.getByRole("grid", { name: "11.2026" }).getByRole("button", { name: "Sunday, 22 November 2026" })).toBeFocused();
        await shot(page, `${layout}-quick-add-calendar-${scheme}`);
        await page.keyboard.press("Escape");
        await page.getByRole("button", { name: "Choose priority" }).click();
        await expect(page.getByRole("option", { name: "Normal", selected: true })).toBeFocused();
        await shot(page, `${layout}-quick-add-priority-${scheme}`);
      }
      await w.check();
    });

    test(`organizing rooms (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page);
      await chooseNew(page, "Collection");
      const input = page.getByRole("textbox", { name: "Collection name" });
      await input.fill("Workshop");
      await expect(input).toBeFocused();
      await shot(page, `organize-new-collection-${scheme}`);
      await input.press("Enter");
      await page.getByRole("button", { name: "Room menu" }).click();
      await page.getByRole("menuitem", { name: "Color…" }).click();
      const collectionColours = page.getByRole("dialog", { name: "Collection color", exact: true });
      await expect(collectionColours.getByRole("option", { selected: true })).toBeFocused();
      await shot(page, `organize-collection-colour-${scheme}`);
      await page.keyboard.press("Escape");
      await page.locator(".sidebar").getByRole("link", { name: /^Planning/ }).click({ button: "right" });
      await page.getByRole("menuitem", { name: "Move to…" }).click();
      await expect(page.getByRole("dialog", { name: "Move to" }).getByRole("option", { selected: true })).toBeFocused();
      await shot(page, `organize-list-move-${scheme}`);
      await page.getByRole("button", { name: "Back", exact: true }).click();
      await page.getByRole("menuitem", { name: "Color…" }).click();
      await expect(page.getByRole("dialog", { name: "List color" }).getByRole("option", { selected: true })).toBeFocused();
      await shot(page, `organize-list-colour-${scheme}`);
      await w.check();
    });

    test(`task Move to picker (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page);
      await page.getByRole("button", { name: "Prepare the November board update", exact: true }).click();
      await page.getByRole("button", { name: "Task actions" }).click();
      await page.getByRole("menuitem", { name: "Move to…" }).click();
      const picker = page.getByRole("dialog", { name: "Move to…" });
      await expect(picker.getByRole("textbox", { name: "Filter lists" })).toBeFocused();
      await expect(picker.getByRole("option", { name: "Planning", selected: true })).toBeDisabled();
      await shot(page, `desktop-task-move-${scheme}`);
      await page.setViewportSize(PHONE);
      await shot(page, `phone-task-move-${scheme}`);
      await w.check();
    });

    test(`task merge picker and confirmation (${scheme})`, async ({ page }) => {
      const w = watch(page);
      for (const [layout, size] of [["desktop", DESKTOP], ["phone", PHONE]]) {
        await page.setViewportSize(size);
        await open(page);
        await page.getByRole("button", { name: "Prepare the November board update", exact: true }).click();
        await page.getByRole("button", { name: "Task actions" }).click();
        await page.getByRole("menuitem", { name: "Merge into…" }).click();
        const picker = page.getByRole("dialog", { name: "Merge into…" });
        const filter = picker.getByRole("textbox", { name: "Filter open tasks" });
        await expect(filter).toBeFocused();
        await filter.fill("plan");
        await expect(picker.getByRole("button", { name: /Outline the 2027 hiring plan/ })).toBeVisible();
        await shot(page, `${layout}-task-merge-picker-${scheme}`);
        await picker.getByRole("button", { name: /Outline the 2027 hiring plan/ }).click();
        await expect(page.getByRole("dialog", { name: "Merge tasks?" }).getByRole("button", { name: "Cancel" })).toBeFocused();
        await shot(page, `${layout}-task-merge-confirm-${scheme}`);
      }
      await w.check();
    });

    test(`detail organizers (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page);
      await page.getByRole("button", { name: "Review the API latency report", exact: true }).click();
      await page.keyboard.press("r");
      const repeat = page.getByRole("dialog", { name: "Repeat", exact: true });
      await expect(repeat.getByRole("option", { name: "Don’t repeat", selected: true })).toBeFocused();
      await shot(page, `organize-repeat-${scheme}`);
      await repeat.getByRole("option", { name: "Custom…" }).click();
      const custom = page.getByRole("dialog", { name: "Custom repeat" });
      await custom.getByRole("spinbutton", { name: "Interval" }).fill("2");
      await custom.getByRole("combobox", { name: "Unit" }).selectOption("Week");
      await custom.getByRole("button", { name: "Tuesday" }).click();
      await expect(custom.getByRole("status")).toContainText("Every 2 weeks");
      await shot(page, `organize-repeat-custom-${scheme}`);
      await page.keyboard.press("Escape");

      const tags = page.getByRole("button", { name: "Tags", exact: true });
      await tags.click();
      const picker = page.getByRole("dialog", { name: "Tags", exact: true });
      const filter = picker.getByRole("textbox", { name: "Type to find or create a tag" });
      await expect(filter).toBeFocused();
      await shot(page, `organize-tags-${scheme}`);
      await filter.fill("research");
      await expect(picker.getByRole("button", { name: /Create tag/ })).toBeVisible();
      await shot(page, `organize-tag-create-${scheme}`);
      await filter.fill("");
      const manage = () => page.getByRole("button", { name: "Manage tag “ops”" });
      await manage().click();
      await expect(page.getByRole("button", { name: "Remove from task" })).toBeFocused();
      await shot(page, `organize-tag-actions-${scheme}`);
      await page.getByRole("button", { name: "Color…", exact: true }).click();
      await expect(page.getByRole("dialog", { name: "Tag color" }).getByRole("option", { selected: true })).toBeFocused();
      await shot(page, `organize-tag-colour-${scheme}`);
      await page.getByRole("button", { name: "Back", exact: true }).click();
      await page.getByRole("button", { name: "Rename tag" }).click();
      await expect(page.getByRole("textbox", { name: "Tag name" })).toBeFocused();
      await shot(page, `organize-tag-rename-${scheme}`);
      await page.keyboard.press("Escape");
      await expect(tags).toBeFocused();
      await tags.click();
      await manage().click();
      await page.getByRole("button", { name: "Delete tag…" }).click();
      await expect(page.getByRole("dialog", { name: "Delete tag" }).getByRole("button", { name: "Cancel" })).toBeFocused();
      await shot(page, `organize-tag-delete-${scheme}`);
      await page.keyboard.press("Escape");
      await expect(tags).toBeFocused();
      await page.keyboard.press("l");
      const link = page.getByRole("textbox", { name: "Add a web link" });
      await link.fill("example.com/report");
      await expect(link).toBeFocused();
      await shot(page, `organize-link-editor-${scheme}`);
      await link.press("Enter");
      await expect(page.locator('a[href="https://example.com/report"]')).toBeVisible();
      await page.getByRole("button", { name: /Remove link/ }).click();
      await expect(page.locator(".toast").filter({ hasText: "Link removed." }).getByRole("button", { name: /^Undo/ })).toBeVisible();
      await shot(page, `organize-link-removed-${scheme}`);
      await w.check();
    });

    test(`reminder presets and custom time (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page);
      await page.getByRole("button", { name: "Prepare the November board update", exact: true }).click();
      const reminder = page.getByRole("button", { name: "Reminder", exact: true });
      const presets = page.getByRole("dialog", { name: "Reminder", exact: true });
      const custom = presets.getByRole("option", { name: "Custom…", selected: true });
      await reminder.press("Enter");
      await expect(custom).toBeFocused();
      await shot(page, `reminder-presets-${scheme}`);
      await page.keyboard.press("Escape");

      await page.setViewportSize(PHONE);
      await reminder.click();
      await custom.press("Enter");
      const editor = page.getByRole("dialog", { name: "Custom reminder", exact: true });
      await expect(editor.getByLabel("Date", { exact: true })).toBeFocused();
      await shot(page, `phone-reminder-custom-${scheme}`);
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

    test(`room menu, delete confirmation and undo (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(DESKTOP);
      await open(page);
      const work = page.locator(".sidebar").getByRole("link", { name: /^Work/ });
      await work.click({ button: "right" });
      await expect(page.getByRole("menuitem", { name: "Rename collection" })).toBeFocused();
      await shot(page, `sidebar-context-menu-${scheme}`);
      await page.keyboard.press("Escape");
      await page.locator(".sidebar").getByRole("link", { name: /^Planning/ }).click();
      const more = page.getByRole("button", { name: "Room menu" });
      await more.press("Enter");
      await expect(page.getByRole("menuitem", { name: "Rename list" })).toBeFocused();
      await shot(page, `room-menu-${scheme}`);
      await page.keyboard.press("Enter");
      const name = page.getByRole("textbox", { name: "list name" });
      await expect(name).toBeFocused();
      await shot(page, `rename-list-${scheme}`);
      await page.keyboard.press("Escape");
      await expect(more).toBeFocused();
      await more.press("Enter");
      await expect(page.getByRole("menuitem", { name: "Rename list" })).toBeFocused();
      await page.keyboard.press("End");
      await page.keyboard.press("Enter");
      const dialog = page.getByRole("dialog", { name: "Delete list" });
      await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
      await shot(page, `delete-list-confirm-${scheme}`);
      await dialog.getByRole("button", { name: "Delete", exact: true }).click();
      await expect(page.getByRole("button", { name: /^Undo/ })).toBeVisible();
      await shot(page, `delete-list-undo-${scheme}`);
      await w.check();
    });

    test(`not synced changes (${scheme})`, async ({ page }) => {
      const w = watch(page);
      await page.setViewportSize(PHONE);
      await open(page, "/app?fixture=not-synced");
      await page.getByRole("button", { name: "Lists", exact: true }).click();
      await page.getByRole("button", { name: "2 couldn't sync" }).click();
      await expect(page.getByRole("dialog", { name: "Not synced" })).toBeVisible();
      await shot(page, `phone-not-synced-${scheme}`);
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
      await expect(page.locator(".empty-line")).toHaveCSS("font-weight", "400");
      await expect(page.locator(".empty-line")).toHaveCSS("font-style", "normal");
      await shot(page, `empty-list-${scheme}`);
      await page.setViewportSize(PHONE);
      await shot(page, `phone-empty-list-${scheme}`);
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
