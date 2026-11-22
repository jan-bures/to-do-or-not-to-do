import { expect, test } from "@playwright/test";
import { open } from "./helpers.mjs";

for (const [width, size, lineHeight] of [[1440, 42, 48], [412, 33, 36]]) {
  test(`room typography uses the bundled Geist faces at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await open(page);
    const title = page.locator(".room-title");
    await expect(title).toHaveCSS("font-size", `${size}px`);
    await expect(title).toHaveCSS("line-height", `${lineHeight}px`);
    await expect(title).toHaveCSS("font-weight", "600");
    await expect(title).toHaveCSS("font-family", /^"?Geist"?,/);

    const eyebrow = page.locator(".eyebrow-lead");
    await expect(eyebrow).toHaveCSS("font-weight", "500");
    await expect(eyebrow).toHaveCSS("font-style", "normal");
    await expect(eyebrow).toHaveCSS("font-family", /^"?Geist"?,/);
    await expect(page.locator(".eyebrow-meta").first()).toHaveCSS("font-family", /Geist Mono/);

    // A CSS family name alone also passes with a fallback font. Require a matching loaded face.
    for (const font of ['400 14px Geist', '500 13px Geist', '600 42px Geist', '400 11px "Geist Mono"']) {
      const faces = await page.evaluate(async (font) => {
        const loaded = await document.fonts.load(font, "Příliš žluťoučký kůň");
        return loaded.map((face) => face.status);
      }, font);
      expect(faces).toEqual(["loaded"]);
    }
  });
}
