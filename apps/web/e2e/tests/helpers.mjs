import { expect } from "@playwright/test";

export async function chooseNew(sidebar, choice) {
  await sidebar.getByRole("button", { name: "New", exact: true }).click();
  await sidebar.getByRole("menuitem", { name: choice, exact: true }).click();
}

export async function openDeviceSettings(page) {
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("menuitem", { name: "Devices…", exact: true }).click();
}

/** Fails the test on any console error, page error or CSP violation. */
export function watch(page) {
  const problems = [];
  page.on("console", (m) => {
    if (m.type() === "error" || /Content.Security.Policy/i.test(m.text())) problems.push(`console: ${m.text()}`);
  });
  page.on("pageerror", (e) => problems.push(`page error: ${e.message}`));
  return {
    async check() {
      const csp = await page.evaluate(() => window.__cspViolations ?? []);
      expect([...problems, ...csp]).toEqual([]);
    },
  };
}

/** Opens a path and waits for the app to render. */
export async function open(page, path = "/app") {
  await page.addInitScript(() => {
    window.__cspViolations = [];
    document.addEventListener("securitypolicyviolation", (e) =>
      window.__cspViolations.push(`csp: ${e.violatedDirective} ${e.blockedURI}`),
    );
  });
  await page.goto(path);
  await page.locator(".room-title").first().waitFor();
  await page.evaluate(() => document.fonts.ready);
}

export const openRows = (page) => page.locator(".task-list-area .rows:not(.completed-rows) > .row");

export async function pickup(page, row) {
  const grip = row.locator(".task-drag-grip");
  await grip.scrollIntoViewIfNeeded();
  const { x, y, width, height } = await grip.boundingBox();
  await page.mouse.move(x + width / 2, y + height / 2);
  await page.mouse.down();
  await page.mouse.move(x + width / 2, y + height / 2 + 8);
  await expect(page.locator(".task-drag-lift")).toBeVisible();
}

export async function dragOver(page, target, fraction = 0.5) {
  await target.scrollIntoViewIfNeeded();
  const { x, y, width, height } = await target.boundingBox();
  await page.mouse.move(x + width / 2, y + height * fraction, { steps: 5 });
}
