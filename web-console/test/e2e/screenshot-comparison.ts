/** The one screenshot comparison contract of every browser reference.
 *
 * Rendering is already fixed by `scripts/browser-tests.sh`: one digest-pinned
 * Linux x86_64 Playwright container, its own fonts, fixed fixtures and clocks
 * and reduced motion. What that authority still leaves is low-amplitude
 * rasterizer noise — Chromium's anti-aliased coverage of glyph edges and
 * rounded corners landing a few intensity levels apart between runs, with
 * identical geometry and content.
 *
 * `threshold` is Playwright's per-pixel perceived colour difference (pixelmatch
 * YIQ: a pixel differs when its weighted squared delta exceeds
 * `35215 * threshold²`). 0.027 is the smallest value that classifies every
 * measured noise pixel in `test/fixtures/rasterizer-noise.json` as equal — the
 * worst needs 0.02652, a uniform shift of 7 grey levels — so a uniform shift
 * of 8 levels or more still differs. `maxDiffPixels` stays 0: a single pixel
 * above that threshold fails the comparison. There are no per-screenshot
 * allowances. `test/screenshot-comparison.test.ts` holds this contract to the
 * measured noise and to real visual changes. */
export const screenshotComparison = { threshold: 0.027, maxDiffPixels: 0 } as const;
