// @vitest-environment node
import { readFileSync } from 'node:fs';
import { expect, it } from 'vitest';
import {
  compareScreenshot,
  decodePng,
  type MeasuredNoisePixel,
  type NoisePolicy,
  type NoiseRegion,
  type Rgba,
  type RgbaImage,
} from './screenshot-comparator';
import manifest from './fixtures/rasterizer-noise.json';

/** The screenshot acceptance contract, held to the pure comparator.
 *
 * `test/fixtures/rasterizer-noise.json` is the explicit exception contract:
 * measured rasterizer noise per reference, grouped into bounded regions with
 * changed-pixel budgets and raw channel-delta bounds. These tests prove both
 * halves of the contract — the measured noise passes, and everything the
 * policy does not explicitly evidence fails: the same delta outside a region,
 * a large-area or whole-image low-amplitude shift (which the removed global
 * `threshold: 0.027` silently accepted), a count or delta beyond a region's
 * bounds, layout shifts, missing elements and dimension changes. */

const noisePolicy = manifest as unknown as NoisePolicy;
const snapshotDirectories = ['agent', 'settings-presentation', 'shell', 'trajectory'].map(spec =>
  new URL(`./e2e/${spec}.spec.ts-snapshots/`, import.meta.url),
);
const NARROW = 'settings-agent-narrow-dark-linux.png';
const INVENTORY = 'settings-inventory-light-linux.png';
const GENERAL = 'settings-general-dark-linux.png';
const reference = (name: string): RgbaImage => {
  for (const directory of snapshotDirectories) {
    try {
      return decodePng(readFileSync(new URL(name, directory)));
    } catch {
      // The manifest references several specs' snapshot directories.
    }
  }
  throw new Error(`No checked-in reference ${name} in any spec snapshots directory`);
};
const copy = (image: RgbaImage): RgbaImage => ({ width: image.width, height: image.height, data: Uint8Array.from(image.data) });
const offset = (image: RgbaImage, x: number, y: number) => (y * image.width + x) * 4;
const pixel = (image: RgbaImage, x: number, y: number): Rgba => [...image.data.slice(offset(image, x, y), offset(image, x, y) + 4)] as Rgba;
const paint = (image: RgbaImage, x: number, y: number, rgba: Rgba) => image.data.set(rgba, offset(image, x, y));
/** Shift a pixel's RGB channels by exactly `±delta` (direction chosen so the
 * value stays in 0..255), leaving alpha alone: an exact, auditable change. */
const nudge = (image: RgbaImage, x: number, y: number, delta: number) => {
  const before = pixel(image, x, y);
  paint(image, x, y, [
    ...before.slice(0, 3).map(value => (value <= 255 - delta ? value + delta : value - delta)),
    before[3],
  ] as Rgba);
};
const compare = (actual: RgbaImage, referenceName: string, expected = reference(referenceName), policy = noisePolicy) =>
  compareScreenshot({ referenceName, expected, actual, noisePolicy: policy });
const entryOf = (referenceName: string) => noisePolicy.find(entry => entry.reference === referenceName)!;
const cells = (region: NoiseRegion): [number, number][] => {
  const found: [number, number][] = [];
  for (let y = region.y; y < region.y + region.height; y++)
    for (let x = region.x; x < region.x + region.width; x++) found.push([x, y]);
  return found;
};

it('the noise manifest is measured evidence bound to bounded, disjoint regions', () => {
  expect(noisePolicy.length).toBeGreaterThan(0);
  for (const entry of noisePolicy) {
    expect(entry.evidence).toBeTruthy();
    expect(entry.regions.length).toBeGreaterThan(0);
    const image = reference(entry.reference);
    for (const region of entry.regions) {
      expect(region.label).toBeTruthy();
      expect(region.width).toBeGreaterThan(0);
      expect(region.height).toBeGreaterThan(0);
      expect(region.x).toBeGreaterThanOrEqual(0);
      expect(region.y).toBeGreaterThanOrEqual(0);
      expect(region.x + region.width).toBeLessThanOrEqual(image.width);
      expect(region.y + region.height).toBeLessThanOrEqual(image.height);
      expect(region.maxChangedPixels).toBeGreaterThan(0);
      expect(region.maxChannelDelta).toBeGreaterThan(0);
    }
    for (const [a, index] of entry.regions.map((region, index) => [region, index] as const))
      for (const b of entry.regions.slice(index + 1)) {
        const overlap = a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
        expect(overlap).toBe(false);
      }
    // Every recorded measurement binds to exactly one region, inside both
    // bounds, and the region's budget covers its own evidence.
    const measured = entry.pixels as MeasuredNoisePixel[];
    expect(measured.length).toBeGreaterThan(0);
    const perRegion = new Map<NoiseRegion, number>();
    for (const [x, y, before, after] of measured) {
      expect(pixel(image, x, y)).toEqual(before);
      expect(after).not.toEqual(before);
      const owners = entry.regions.filter(region => x >= region.x && x < region.x + region.width && y >= region.y && y < region.y + region.height);
      expect(owners).toHaveLength(1);
      const delta = Math.max(...before.map((channel, i) => Math.abs(after[i] - channel)));
      expect(delta).toBeLessThanOrEqual(owners[0].maxChannelDelta);
      perRegion.set(owners[0], (perRegion.get(owners[0]) ?? 0) + 1);
    }
    for (const region of entry.regions) expect(perRegion.get(region) ?? 0).toBeLessThanOrEqual(region.maxChangedPixels);
  }
});

it('A: an exact baseline passes with and without a noise policy', () => {
  for (const name of [NARROW, INVENTORY, GENERAL]) {
    const expected = reference(name);
    const result = compare(copy(expected), name);
    expect(result.ok).toBe(true);
    expect(result.totalChanged).toBe(0);
    expect(result.changedOutsideRegions).toBe(0);
  }
});

it.each(noisePolicy.map(entry => [entry.reference, entry] as const))(
  'B: the recorded rasterizer noise of %s passes, and only the registered policy accepts it',
  (name, entry) => {
    const expected = reference(name);
    const actual = copy(expected);
    for (const [x, y, before, after] of entry.pixels as MeasuredNoisePixel[]) {
      // The manifest describes exactly this reference; a changed reference must
      // be re-measured rather than silently accepted.
      expect(pixel(expected, x, y)).toEqual(before);
      paint(actual, x, y, after);
    }
    const result = compare(actual, name);
    expect(result.ok).toBe(true);
    expect(result.totalChanged).toBe((entry.pixels as MeasuredNoisePixel[]).length);
    expect(result.changedOutsideRegions).toBe(0);
    // Without the manifest entry the identical capture is a failure: the
    // differences are real and tolerance is evidence, not inference.
    expect(compare(actual, name, expected, []).ok).toBe(false);
  },
);

it('C: noise may move to unrecorded coordinates inside its approved region, within both bounds', () => {
  const expected = reference(NARROW);
  const recorded = new Set((entryOf(NARROW).pixels as MeasuredNoisePixel[]).map(([x, y]) => `${x},${y}`));
  const region = entryOf(NARROW).regions[0]; // 9x3 corner cell, budget 9, delta bound 3
  const candidates = cells(region).filter(([x, y]) => !recorded.has(`${x},${y}`));
  expect(candidates.length).toBeGreaterThanOrEqual(8);
  const actual = copy(expected);
  for (const [x, y] of candidates.slice(0, 8)) nudge(actual, x, y, 3);
  const result = compare(actual, NARROW);
  expect(result.ok).toBe(true);
  expect(result.changedOutsideRegions).toBe(0);
  expect(result.regions[0].changedPixels).toBe(8);
  // The same freedom inside the light Settings evidence regions.
  const light = reference(INVENTORY);
  const lightRecorded = new Set((entryOf(INVENTORY).pixels as MeasuredNoisePixel[]).map(([x, y]) => `${x},${y}`));
  const cog = entryOf(INVENTORY).regions[0]; // budget 17, delta bound 7
  const cogCandidates = cells(cog).filter(([x, y]) => !lightRecorded.has(`${x},${y}`));
  const lightActual = copy(light);
  for (const [x, y] of cogCandidates.slice(0, 17)) nudge(lightActual, x, y, 5);
  const lightResult = compare(lightActual, INVENTORY);
  expect(lightResult.ok).toBe(true);
  expect(lightResult.changedOutsideRegions).toBe(0);
});

it('D: the same low-amplitude change outside every approved region fails', () => {
  const expected = reference(NARROW);
  expect(pixel(expected, 200, 700)).toEqual([53, 54, 56, 255]); // flat panel, far from any edge
  for (const delta of [1, 2, 3, 7]) {
    const actual = copy(expected);
    nudge(actual, 200, 700, delta);
    const result = compare(actual, NARROW);
    expect(result.ok).toBe(false);
    expect(result.changedOutsideRegions).toBe(1);
    expect(result.report).toContain('outside every approved noise region');
  }
  // The measured tolerated magnitude, applied to a flat panel pixel of the
  // light reference, outside its registered cog/corner regions.
  const light = reference(INVENTORY);
  expect(pixel(light, 400, 400)).toEqual([255, 255, 255, 255]);
  const shifted = copy(light);
  nudge(shifted, 400, 400, 7);
  const lightResult = compare(shifted, INVENTORY);
  expect(lightResult.ok).toBe(false);
  expect(lightResult.changedOutsideRegions).toBe(1);
  expect(lightResult.maxObservedDelta).toBe(7);
  // A reference with no manifest entry is strict: one changed pixel fails.
  const strict = copy(reference(GENERAL));
  nudge(strict, 400, 400, 1);
  const strictResult = compare(strict, GENERAL);
  expect(strictResult.ok).toBe(false);
  expect(strictResult.report).toContain('no noise regions are registered');
  // Reference-locality: the inventory evidence gives an unrelated screenshot
  // zero tolerance, even at the exact approved coordinates and delta.
  const unrelated = copy(reference(GENERAL));
  const inventoryEntry = entryOf(INVENTORY);
  for (const [x, y] of (inventoryEntry.pixels as MeasuredNoisePixel[]).map(([x, y]) => [x, y] as const))
    nudge(unrelated, x, y, inventoryEntry.regions.find(r => x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)!.maxChannelDelta);
  const unrelatedResult = compare(unrelated, GENERAL);
  expect(unrelatedResult.ok).toBe(false);
  expect(unrelatedResult.changedOutsideRegions).toBe((inventoryEntry.pixels as MeasuredNoisePixel[]).length);
});

it('E: a large-area RGB +7 regression fails (the defect the old global threshold accepted)', () => {
  const expected = reference(NARROW);
  // A 228x252 flat panel rectangle, far from every approved noise region.
  const actual = copy(expected);
  let flat = 0;
  for (let y = 564; y <= 815; y++)
    for (let x = 117; x <= 344; x++) {
      const [r, g, b] = pixel(actual, x, y);
      if (r === 53 && g === 54 && b === 56) flat++;
      nudge(actual, x, y, 7);
    }
  expect(flat).toBe(228 * 252);
  const result = compare(actual, NARROW, expected, noisePolicy);
  // The removed contract (per-pixel threshold 0.027, maxDiffPixels 0)
  // classified every one of these 7-level shifts as identical and passed.
  expect(result.totalChanged).toBeGreaterThanOrEqual(50_000);
  expect(result.changedOutsideRegions).toBe(result.totalChanged);
  expect(result.ok).toBe(false);
  expect(result.firstUnexpected.length).toBeLessThanOrEqual(10);
  // Failure artifacts stay bounded and mark the unexpected area red.
  const diffed = compareScreenshot({ referenceName: NARROW, expected, actual, noisePolicy, renderDiff: true });
  expect(diffed.ok).toBe(false);
  expect(diffed.diff).toBeDefined();
  expect(diffed.diff!.width).toBe(expected.width);
  expect(pixel(diffed.diff!, 117, 564)).toEqual([255, 0, 0, 255]);
});

it('F: a whole-image low-amplitude shift fails (the defect the old global threshold accepted)', () => {
  const expected = reference(INVENTORY);
  const actual = copy(expected);
  let shiftable = 0;
  for (let y = 0; y < expected.height; y++)
    for (let x = 0; x < expected.width; x++) {
      const [r, g, b] = pixel(expected, x, y);
      if (r >= 2 && g >= 2 && b >= 2) {
        shiftable++;
        nudge(actual, x, y, -2);
      }
    }
  expect(shiftable).toBe(expected.width * expected.height);
  const result = compare(actual, INVENTORY);
  // The old contract accepted any per-pixel delta below its threshold
  // regardless of area; this whole-image 2-level shift passed it.
  expect(result.totalChanged).toBe(shiftable);
  expect(result.changedOutsideRegions).toBeGreaterThan(600_000);
  expect(result.ok).toBe(false);
});

it('G: a changed-pixel count beyond an approved region budget fails', () => {
  const expected = reference(NARROW);
  const region = entryOf(NARROW).regions[0]; // budget 9, delta bound 3
  const recorded = new Set((entryOf(NARROW).pixels as MeasuredNoisePixel[]).map(([x, y]) => `${x},${y}`));
  const candidates = cells(region).filter(([x, y]) => !recorded.has(`${x},${y}`));
  // Exactly the budget passes …
  const atBudget = copy(expected);
  for (const [x, y] of candidates.slice(0, region.maxChangedPixels)) nudge(atBudget, x, y, 1);
  const accepted = compare(atBudget, NARROW);
  expect(accepted.ok).toBe(true);
  expect(accepted.regions[0].changedPixels).toBe(region.maxChangedPixels);
  // … one qualifying low-delta pixel more fails.
  const overBudget = copy(expected);
  for (const [x, y] of candidates.slice(0, region.maxChangedPixels + 1)) nudge(overBudget, x, y, 1);
  const rejected = compare(overBudget, NARROW);
  expect(rejected.ok).toBe(false);
  expect(rejected.regions[0].changedPixels).toBe(region.maxChangedPixels + 1);
  expect(rejected.report).toContain(`exceed the approved budget ${region.maxChangedPixels}`);
});

it('H: a channel delta beyond an approved region bound fails', () => {
  const narrow = reference(NARROW);
  const corner = entryOf(NARROW).regions[0]; // delta bound 3
  const overDelta = copy(narrow);
  nudge(overDelta, corner.x, corner.y, corner.maxChannelDelta + 1);
  const narrowResult = compare(overDelta, NARROW);
  expect(narrowResult.ok).toBe(false);
  expect(narrowResult.regions[0].overDeltaPixels).toBe(1);
  expect(narrowResult.report).toContain(`exceeds the approved bound ${corner.maxChannelDelta}`);
  // Even with an in-budget pixel count, an arbitrary colour change inside the
  // coordinate box is not rasterizer noise.
  const light = reference(INVENTORY);
  const cog = entryOf(INVENTORY).regions[0]; // delta bound 7
  const lightOver = copy(light);
  nudge(lightOver, cog.x, cog.y, cog.maxChannelDelta + 1);
  const lightResult = compare(lightOver, INVENTORY);
  expect(lightResult.ok).toBe(false);
  expect(lightResult.report).toContain(`exceeds the approved bound ${cog.maxChannelDelta}`);
});

it('I: a one-pixel layout shift fails', () => {
  const expected = reference(GENERAL);
  const actual = copy(expected);
  for (let y = 0; y < expected.height; y++)
    for (let x = expected.width - 1; x > 0; x--) paint(actual, x, y, pixel(expected, x - 1, y));
  expect(compare(actual, GENERAL).ok).toBe(false);
});

it('J: a missing element fails', () => {
  const expected = reference(GENERAL);
  const actual = copy(expected);
  // Paint the "Runtime · General" heading out with the surface behind it.
  const surface = pixel(expected, 205, 415);
  let glyphs = 0;
  for (let y = 418; y < 440; y++)
    for (let x = 212; x < 345; x++) {
      if (pixel(expected, x, y).some((channel, index) => channel !== surface[index])) glyphs++;
      paint(actual, x, y, surface);
    }
  expect(glyphs).toBeGreaterThan(0);
  expect(compare(actual, GENERAL).ok).toBe(false);
});

it('K: a dimension change fails, for strict and noise-policy references alike', () => {
  const expected = reference(GENERAL);
  const narrow = reference(NARROW);
  const cropWidth = (image: RgbaImage, width: number): RgbaImage => {
    const data = new Uint8Array(width * image.height * 4);
    for (let y = 0; y < image.height; y++) data.set(image.data.subarray(y * image.width * 4, y * image.width * 4 + width * 4), y * width * 4);
    return { width, height: image.height, data };
  };
  const cropHeight = (image: RgbaImage, height: number): RgbaImage => ({
    width: image.width,
    height,
    data: image.data.slice(0, height * image.width * 4),
  });
  const widthResult = compare(cropWidth(expected, expected.width - 1), GENERAL);
  expect(widthResult.ok).toBe(false);
  expect(widthResult.report).toContain(`dimensions: expected 800x800, actual 799x800`);
  expect(widthResult.report).toContain('geometry must match exactly');
  const heightResult = compare(cropHeight(narrow, narrow.height - 1), NARROW);
  expect(heightResult.ok).toBe(false);
  expect(heightResult.report).toContain(`dimensions: expected 390x844, actual 390x843`);
});
