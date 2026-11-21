import { expect } from "@playwright/test";

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
