// Real server pages, filled through their real endpoints with invented, disposable records.
import { expect, test } from "@playwright/test";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { fileURLToPath } from "node:url";
import { GALLERY_DATA } from "../live-env.mjs";

const gallery = resolve(dirname(fileURLToPath(import.meta.url)), "../../../../docs/mockups/admin");
const list = "0193b2c0-0000-7000-8000-000000000001";
const longName = "Living-room tablet (kitchen dock, family calendar display)";
const reviewCrops = {
  overview: { banner: ".session-note" },
  devices: { metadata: "#paired-apps + a + .rows .facts" },
  agents: { metadata: ".row .facts", select: ".form-grid .select-control" },
  "agent-grants": { select: ".grant .select-control" },
  shares: { select: ".select-control" },
  email: { select: ".select-control" },
  audit: { row: ".audit-line" },
};

function ageAuditFixtures() {
  const db = new DatabaseSync(join(GALLERY_DATA, "todo.sqlite3"));
  const today = new Date().setUTCHours(0, 0, 0, 0);
  const minute = 60_000;
  const day = 24 * 60 * minute;
  try {
    const move = db.prepare("UPDATE audit SET at = ? WHERE kind = ?");
    move.run(today - day + (16 * 60 + 18) * minute, "agent_token_created");
    move.run(today - 3 * day + (9 * 60 + 42) * minute, "setup_completed");
  } finally {
    db.close();
  }
}

async function send(page, path, fields, form = false, csrf = true) {
  const res = await page.evaluate(async ({ path, fields, form, csrf }) => {
    const body = form ? new URLSearchParams(fields) : JSON.stringify(fields);
    const headers = { "content-type": form ? "application/x-www-form-urlencoded" : "application/json" };
    if (csrf) {
      const token = document.querySelector('meta[name="csrf-token"]').content;
      if (form) body.set("csrf", token);
      else headers["x-csrf-token"] = token;
    }
    const response = await fetch(path, { method: "POST", headers, body });
    return { status: response.status, text: await response.text() };
  }, { path, fields, form, csrf });
  expect(res.status, path).toBeLessThan(300);
  return form ? res.text : JSON.parse(res.text);
}

async function crop(page, cdp, control, width, path) {
  const box = await control.boundingBox();
  const scroll = await page.evaluate(() => scrollY);
  const x = Math.max(0, box.x - 24);
  const y = Math.max(0, box.y + scroll - 24);
  const { data } = await cdp.send("Page.captureScreenshot", {
    format: "png", captureBeyondViewport: true,
    clip: { x, y, width: Math.min(width - x, box.width + 48), height: box.height + 48, scale: 3 },
  });
  writeFileSync(path, Buffer.from(data, "base64"));
}

async function capture(page, testInfo, name, path, focusSelector, nav, editFixture) {
  const cdp = await page.context().newCDPSession(page);
  const problems = [];
  const consoleCheck = (msg) => {
    if (/content.security.policy/i.test(msg.text())) problems.push(msg.text());
  };
  page.on("console", consoleCheck);
  try {
    for (const colorScheme of ["light", "dark"]) {
      for (const [width, height] of [[1440, 900], [412, 915]]) {
        const label = `${name}-${colorScheme}-${width}`;
        await page.emulateMedia({ colorScheme });
        await page.setViewportSize({ width, height });
        // The invite script removes the fragment. A repeat to the same path would otherwise
        // only update the fragment, without loading the page or rerunning the script.
        if (name === "invite") await page.goto("about:blank");
        const response = await page.goto(path);
        expect(response.status(), label).toBe(200);
        expect(response.headers()["cache-control"], label).toBe("no-store, no-transform");
        if (nav) await expect(page.locator('nav [aria-current="page"]')).toHaveText(nav);
        if (editFixture) await editFixture(page);
        const { fits, scheme } = await page.evaluate(async () => {
          await document.fonts.ready;
          return {
            fits: document.documentElement.scrollWidth === innerWidth,
            scheme: getComputedStyle(document.documentElement).colorScheme,
          };
        });
        expect(fits, `${label}: horizontal overflow`).toBe(true);
        expect(scheme, label).toBe(colorScheme);
        await expect(page.locator("h1")).toHaveCSS("font-family", /^"?Geist"?,/);
        expect(await page.evaluate(() => document.fonts.check("600 33px Geist"))).toBe(true);
        for (const separator of await page.locator("main .fact-separator").all()) {
          await expect(separator).toHaveCSS("margin-inline-start", "4px");
          await expect(separator).toHaveCSS("margin-inline-end", "4px");
        }
        if (name === "devices" && width === 412) {
          const heading = page.getByRole("heading", { name: longName, exact: true });
          expect(await heading.evaluate((el) => el.offsetHeight > parseFloat(getComputedStyle(el).lineHeight))).toBe(true);
        }
        if (name === "audit") await expect(page.locator(".audit-line").first()).toHaveCSS("height", "36px");
        for (const select of await page.locator("main select").all()) {
          await expect(select).toHaveCSS("appearance", "none");
          await expect(select).toHaveCSS("height", "36px");
        }
        await page.screenshot({ path: join(gallery, `${label}.png`), fullPage: true });
        for (const [part, selector] of Object.entries(reviewCrops[name] ?? {})) {
          await crop(page, cdp, page.locator(selector).first(), width, testInfo.outputPath(`${label}-${part}-300.png`));
        }

        const control = page.locator(focusSelector).first();
        // A keyboard interaction exposes the actual focus-visible ring, even following a click.
        await page.keyboard.press("Tab");
        await control.focus();
        await expect(control).toHaveCSS("outline-style", "solid");
        await crop(page, cdp, control, width, testInfo.outputPath(`${label}-focus-300.png`));
      }
    }
    expect(problems).toEqual([]);
  } finally {
    page.off("console", consoleCheck);
    await cdp.detach();
  }
}

test("server admin gallery: every page in both schemes at desktop and phone widths", async ({ page, browser }, testInfo) => {
  mkdirSync(gallery, { recursive: true });
  mkdirSync(testInfo.outputDir, { recursive: true });
  const shot = (name, path, selector, nav, edit) => capture(page, testInfo, name, path, selector, nav, edit);
  await shot("login-first-run", "/login", "main button");
  await shot("setup", "/setup", 'input[name="code"]');

  const cdp = await page.context().newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  const options = {
    protocol: "ctap2", transport: "internal", hasResidentKey: true,
    hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true,
  };
  const { authenticatorId } = await cdp.send("WebAuthn.addVirtualAuthenticator", { options });
  const code = readFileSync(join(GALLERY_DATA, "setup-code.txt"), "utf8").trim().split(/\s+/)[0];
  await page.locator('input[name="code"]').fill(code);
  await page.getByRole("button", { name: "Register passkey" }).click();
  await page.waitForURL((u) => u.pathname !== "/setup");
  await page.goto("/admin");
  await cdp.send("WebAuthn.removeVirtualAuthenticator", { authenticatorId });
  await cdp.send("WebAuthn.addVirtualAuthenticator", { options });
  await page.locator("#passkey-name").fill("YubiKey 5C");
  await page.getByRole("button", { name: "Add a passkey" }).click();
  await expect(page.getByRole("heading", { name: "YubiKey 5C" })).toBeVisible();

  const command = await send(page, "/api/v1/commands", {
    command_id: "0193b2c0-0000-7000-8000-0000000000aa",
    command: { cmd: "create_list", id: list, name: "Field notes", color: "lagoon" },
  });
  expect(command).toBeTruthy();
  await send(page, "/admin/agents", { name: "weekly-review", default: "read", writes_per_hour: "30" }, true);
  await send(page, "/admin/shares", { target: `list:${list}` }, true);
  await send(page, "/admin/ingest/inbox", { list }, true);
  await send(page, "/admin/ingest/senders", { address: "you@example.com" }, true);
  await send(page, "/admin/ingest/keys", { key: "notes", list }, true);

  const devices = [];
  for (const name of ["Pixel test phone", "Work laptop", longName]) {
    const pair = await send(page, "/api/v1/pair/start", { device_name: name }, false, false);
    await send(page, "/pair/approve", { code: pair.user_code }, true);
    const device = await send(page, "/api/v1/pair/poll", { poll_token: pair.poll_token }, false, false);
    devices.push(device);
    if (name === "Pixel test phone") {
      const res = await page.request.get("/api/v1/lists", { headers: { authorization: `Bearer ${device.token}` } });
      expect(res.ok()).toBe(true);
    }
  }

  await shot("overview", "/admin", "#passkey-name", "Overview");
  await shot("devices", "/admin/devices", ".confirm summary", "Devices");
  await shot("devices-details", "/admin/devices", ".confirm button", "Devices", async (p) => {
    const pixel = p.getByRole("listitem").filter({ has: p.getByRole("heading", { name: "Pixel test phone" }) });
    await pixel.locator(".row-copy summary").click();
    await p.locator(".confirm summary").first().click();
  });
  const pixel = page.getByRole("listitem").filter({ has: page.getByRole("heading", { name: "Pixel test phone" }) });
  await expect(pixel.locator(".row-copy details code:not(.short-id)")).toHaveText(devices[0].device_id);
  // Submit the rendered destructive form and verify the row changes, including the ID staying
  // visible only when disclosed. No direct API call stands in for the user action.
  const laptop = page.getByRole("listitem").filter({ has: page.getByRole("heading", { name: "Work laptop" }) });
  await laptop.getByText("Revoke", { exact: true }).click();
  await laptop.getByRole("button", { name: "Confirm revoke" }).click();
  await expect(laptop).toContainText("Revoked");

  await shot("agents", "/admin/agents", 'input[name="name"]', "Agents");
  const grants = await page.getByRole("link", { name: "Edit grants" }).getAttribute("href");
  await shot("agent-grants", grants, ".grant select", "Agents");
  await shot("shares", "/admin/shares", 'select[name="target"]', "Share links");
  await shot("email", "/admin/ingest", 'input[type="email"]', "Email");
  ageAuditFixtures();
  await shot("audit", "/admin/audit", "main summary", "Audit log");
  await expect(page.getByRole("region", { name: "Today" })).toBeVisible();
  await expect(page.getByRole("region", { name: "Yesterday" })).toContainText("16:18");
  const event = page.locator(".audit-line").first();
  await expect(event.locator("time")).toHaveAttribute("title", /^\d{4}-\d\d-\d\d \d\d:\d\d:\d\d UTC$/);
  await event.press("Enter");
  await expect(page.locator(".audit-detail").first()).toBeVisible();
  await shot("pair", "/pair", 'input[name="code"]');
  const pending = await send(page, "/api/v1/pair/start", { device_name: "Kitchen tablet (test)" }, false, false);
  // Never publish the generated pairing code, even though it works only on this temporary server.
  await shot("pair-approval", `/pair?code=${pending.user_code}`, 'form[action="/pair/approve"] button', undefined, async (p) => {
    await p.locator("main code").evaluate((el) => { el.textContent = "ABCD-2345"; });
  });
  await shot("verify", "/auth/verify?next=/admin/devices", "main button", "Devices");
  const invite = await send(page, "/api/v1/pair/invites", {});
  const guest = await browser.newContext();
  try {
    const anon = await guest.newPage();
    const base = new URL(page.url());
    await capture(anon, testInfo, "login", new URL("/login", base).href, "main button");
    await capture(anon, testInfo, "pair-sign-in", new URL("/pair", base).href, "main button");
    await capture(anon, testInfo, "invite", invite.link, "#invite-device-name");

    // Invited browsers do not have a fresh assertion yet. Show the alternate verification banner.
    const auth = await guest.newCDPSession(anon);
    await auth.send("WebAuthn.enable");
    await auth.send("WebAuthn.addVirtualAuthenticator", { options });
    await anon.locator("#invite-device-name").fill("Fixture browser");
    await anon.getByRole("button", { name: "Register passkey" }).click();
    await anon.waitForURL((u) => u.pathname !== "/pair/invite");
    await capture(anon, testInfo, "step-up-needed", new URL("/admin/devices", base).href, '.session-note button', "Devices");
  } finally {
    await guest.close();
  }
});
