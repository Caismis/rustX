// @vitest-environment node
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { expect, it } from 'vitest';
import { screenshotComparison } from './e2e/screenshot-comparison';
import noise from './fixtures/rasterizer-noise.json';

/** The screenshot comparison contract, held to Playwright's own comparator.
 *
 * `rasterizer-noise.json` records every pixel that differed in the real
 * rasterizer-noise failures of the pinned browser authority: the reference
 * value and the value Chromium produced instead. Applying those pixels to the
 * checked-in reference reproduces the failing capture exactly. */

type Rgba = [number, number, number, number];
interface PngImage { width: number; height: number; data: Buffer }
const load = createRequire(import.meta.url);
const core = load.resolve('playwright-core/package.json', { paths: [load.resolve('@playwright/test')] }).replace(/package\.json$/, '');
const { utils } = load(`${core}lib/coreBundle.js`) as {
  utils: { getComparator(mimeType: string): (actual: Buffer, expected: Buffer, options: object) => { errorMessage: string } | null };
};
const { PNG } = load(`${core}lib/utilsBundle.js`) as {
  PNG: { sync: { read(buffer: Buffer): PngImage; write(image: PngImage): Buffer } };
};
const compare = utils.getComparator('image/png');
const references = new URL('./e2e/settings-presentation.spec.ts-snapshots/', import.meta.url);
const reference = (name: string) => PNG.sync.read(readFileSync(new URL(name, references)));
const encode = (image: PngImage) => PNG.sync.write(image);
const pixel = (image: PngImage, x: number, y: number): Rgba => [...image.data.subarray((y * image.width + x) * 4, (y * image.width + x) * 4 + 4)] as Rgba;
const paint = (image: PngImage, x: number, y: number, rgba: Rgba) => image.data.set(rgba, (y * image.width + x) * 4);
const copy = (image: PngImage): PngImage => ({ width: image.width, height: image.height, data: Buffer.from(image.data) });
const matches = (actual: PngImage, expected: PngImage, options: object = screenshotComparison) =>
  compare(encode(actual), encode(expected), options) === null;

it.each(noise.map(entry => [entry.reference, entry] as const))('measured rasterizer noise on %s compares equal', (name, entry) => {
  const expected = reference(name);
  const actual = copy(expected);
  for (const [x, y, before, after] of entry.pixels as [number, number, Rgba, Rgba][]) {
    // The fixture describes exactly this reference; a changed reference must
    // be re-measured rather than silently accepted.
    expect(pixel(expected, x, y)).toEqual(before);
    paint(actual, x, y, after);
  }
  // The former zero-threshold contract failed on this capture ...
  expect(matches(actual, expected, { threshold: 0, maxDiffPixels: 0 })).toBe(false);
  // ... and the current contract classifies it as the same rendering.
  expect(matches(actual, expected)).toBe(true);
});

it('a colour change just above the measured noise fails on a single pixel', () => {
  const expected = reference('settings-agent-narrow-dark-linux.png');
  // A flat panel pixel, far from any edge, so no anti-aliasing heuristic applies.
  const [r, g, b, a] = pixel(expected, 200, 700);
  for (const [shift, equal] of [[7, true], [8, false]] as const) {
    const actual = copy(expected);
    paint(actual, 200, 700, [r + shift, g + shift, b + shift, a]);
    expect(matches(actual, expected)).toBe(equal);
  }
});

it('a one-pixel layout shift fails', () => {
  const expected = reference('settings-general-dark-linux.png');
  const actual = copy(expected);
  for (let y = 0; y < expected.height; y++)
    for (let x = expected.width - 1; x > 0; x--) paint(actual, x, y, pixel(expected, x - 1, y));
  expect(matches(actual, expected)).toBe(false);
});

it('a missing element fails', () => {
  const expected = reference('settings-general-dark-linux.png');
  const actual = copy(expected);
  // Paint the "Runtime · General" heading out with the surface behind it.
  const surface = pixel(expected, 205, 415);
  let glyphs = 0;
  for (let y = 418; y < 440; y++) for (let x = 212; x < 345; x++) {
    if (pixel(expected, x, y).some((channel, index) => channel !== surface[index])) glyphs++;
    paint(actual, x, y, surface);
  }
  expect(glyphs).toBeGreaterThan(0);
  expect(matches(actual, expected)).toBe(false);
});
