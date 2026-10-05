// @vitest-environment node
import { createHash } from 'node:crypto';
import { readdirSync, readFileSync } from 'node:fs';
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
import frozenManifest from './fixtures/screenshot-comparator/noise.json';

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
const e2e = new URL('./e2e/', import.meta.url);
const snapshotDirectories = readdirSync(e2e)
  .filter(entry => entry.endsWith('.spec.ts-snapshots'))
  .map(entry => new URL(`${entry}/`, e2e));
/** The comparator's own contract is proven against frozen fixtures: earlier
 * Settings and Composer references and their measured rasterizer noise. They are copies, not live references, so a product change to a live
 * screenshot never silently rewrites the comparator's evidence. */
const comparatorPolicy = frozenManifest as unknown as NoisePolicy;
const frozen = new URL('./fixtures/screenshot-comparator/', import.meta.url);
const fixture = (name: string): RgbaImage => decodePng(readFileSync(new URL(name, frozen)));
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
const compare = (actual: RgbaImage, referenceName: string, expected = fixture(referenceName), policy = comparatorPolicy) =>
  compareScreenshot({ referenceName, expected, actual, noisePolicy: policy });
const entryOf = (referenceName: string) => comparatorPolicy.find(entry => entry.reference === referenceName)!;
const cells = (region: NoiseRegion): [number, number][] => {
  const found: [number, number][] = [];
  for (let y = region.y; y < region.y + region.height; y++)
    for (let x = region.x; x < region.x + region.width; x++) found.push([x, y]);
  return found;
};

it.each([
  ['live', noisePolicy, reference],
  ['frozen comparator', comparatorPolicy, fixture],
] as const)('the %s noise manifest is measured evidence bound to bounded, disjoint regions', (_label, policy, image_) => {
  expect(policy.length).toBeGreaterThan(0);
  for (const entry of policy) {
    expect(entry.evidence).toBeTruthy();
    expect(entry.regions.length).toBeGreaterThan(0);
    const image = image_(entry.reference);
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
    const expected = fixture(name);
    const result = compare(copy(expected), name);
    expect(result.ok).toBe(true);
    expect(result.totalChanged).toBe(0);
    expect(result.changedOutsideRegions).toBe(0);
  }
});

it.each([
  ...noisePolicy.map(entry => [entry.reference, entry, reference(entry.reference), noisePolicy] as const),
  ...comparatorPolicy.map(entry => [`frozen ${entry.reference}`, entry, fixture(entry.reference), comparatorPolicy] as const),
])(
  'B: the recorded rasterizer noise of %s passes, and only the registered policy accepts it',
  (_label, entry, expected, policy) => {
    const name = entry.reference;
    const actual = copy(expected);
    for (const [x, y, before, after] of entry.pixels as MeasuredNoisePixel[]) {
      // The manifest describes exactly this reference; a changed reference must
      // be re-measured rather than silently accepted.
      expect(pixel(expected, x, y)).toEqual(before);
      paint(actual, x, y, after);
    }
    const result = compare(actual, name, expected, policy);
    expect(result.ok).toBe(true);
    expect(result.totalChanged).toBe((entry.pixels as MeasuredNoisePixel[]).length);
    expect(result.changedOutsideRegions).toBe(0);
    // Without the manifest entry the identical capture is a failure: the
    // differences are real and tolerance is evidence, not inference.
    expect(compare(actual, name, expected, []).ok).toBe(false);
  },
);

it('C: noise may move to unrecorded coordinates inside its approved region, within both bounds', () => {
  const expected = fixture(NARROW);
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
  const light = fixture(INVENTORY);
  const lightRecorded = new Set((entryOf(INVENTORY).pixels as MeasuredNoisePixel[]).map(([x, y]) => `${x},${y}`));
  const cog = entryOf(INVENTORY).regions[0]; // budget 17, delta bound 7
  const cogCandidates = cells(cog).filter(([x, y]) => !lightRecorded.has(`${x},${y}`));
  const lightActual = copy(light);
  for (const [x, y] of cogCandidates.slice(0, 17)) nudge(lightActual, x, y, 5);
  const lightResult = compare(lightActual, INVENTORY);
  expect(lightResult.ok).toBe(true);
  expect(lightResult.changedOutsideRegions).toBe(0);
});

it('composer corner evidence grants tolerance to exactly its 16 measured sites and bounds', () => {
  const name = 'composer-running-draft-dark-390-linux.png';
  const expected = fixture(name);
  const entry = comparatorPolicy.find(entry => entry.reference === name)!;
  const measured = entry.pixels as MeasuredNoisePixel[];
  const sites = entry.regions.flatMap(cells);
  expect(sites).toHaveLength(16);
  expect(sites).toEqual(measured.map(([x, y]) => [x, y]));
  const observed = copy(expected);
  for (const [x, y, , after] of measured) paint(observed, x, y, after);
  expect(compare(observed, name, expected, comparatorPolicy).ok).toBe(true);
  // The same capture has no allowance under another reference's name.
  expect(compare(observed, 'composer-idle-draft-dark-390-linux.png', expected, comparatorPolicy).ok).toBe(false);
  for (const region of entry.regions) {
    const excessive = copy(expected);
    nudge(excessive, region.x, region.y, region.maxChannelDelta + 1);
    expect(compare(excessive, name, expected, comparatorPolicy).ok).toBe(false);
    // Neither neighboring pixels nor an extra row inherit the corner's bound.
    for (const [x, y] of [[region.x - 1, region.y], [region.x + region.width, region.y], [region.x, 75]]) {
      const adjacent = copy(observed);
      nudge(adjacent, x, y, 1);
      const result = compare(adjacent, name, expected, comparatorPolicy);
      expect(result.ok).toBe(false);
      expect(result.changedOutsideRegions).toBe(1);
    }
  }
});

it('composer corner noise cannot conceal text, focus contrast, theme or layout changes', () => {
  const name = 'composer-running-draft-dark-390-linux.png';
  const expected = fixture(name);
  const entry = comparatorPolicy.find(entry => entry.reference === name)!;
  const observed = copy(expected);
  for (const [x, y, , after] of entry.pixels as MeasuredNoisePixel[]) paint(observed, x, y, after);
  for (const [x, y] of [[50, 95], [150, 71]]) {
    const changed = copy(observed);
    nudge(changed, x, y, 1);
    expect(compare(changed, name, expected, comparatorPolicy).ok).toBe(false);
  }
  const theme = copy(observed);
  for (let y = 0; y < theme.height; y++)
    for (let x = 0; x < theme.width; x++) nudge(theme, x, y, 1);
  expect(compare(theme, name, expected, comparatorPolicy).ok).toBe(false);
  const shifted = copy(observed);
  for (let y = 0; y < shifted.height; y++)
    for (let x = shifted.width - 1; x > 0; x--) paint(shifted, x, y, pixel(observed, x - 1, y));
  expect(compare(shifted, name, expected, comparatorPolicy).ok).toBe(false);
  expect(compare({ ...observed, height: observed.height - 1 }, name, expected, comparatorPolicy).ok).toBe(false);
});

it.each(['light', 'dark'] as const)('live running-draft %s corner policy grants only exact measured sites and maxima', theme => {
  const name = `composer-running-draft-${theme}-390-linux.png`, expected = reference(name);
  const entry = noisePolicy.find(entry => entry.reference === name)!;
  const measured = entry.pixels as MeasuredNoisePixel[];
  expect(measured).toHaveLength(theme === 'light' ? 15 : 16);
  expect(entry.regions.map(region => [region.x, region.y])).toEqual(measured.map(([x, y]) => [x, y]));
  const observed = copy(expected), registered = new Set(measured.map(([x, y]) => `${x},${y}`));
  for (const [x, y, , after] of measured) paint(observed, x, y, after);
  expect(compare(observed, name, expected, noisePolicy).ok).toBe(true);
  expect(compare(observed, name, expected, []).ok).toBe(false);
  for (const [index, region] of entry.regions.entries()) {
    const [, , before, after] = measured[index];
    expect([region.width, region.height, region.maxChangedPixels]).toEqual([1, 1, 1]);
    expect(region.maxChannelDelta).toBe(Math.max(...before.map((value, channel) => Math.abs(value - after[channel]))));
    const excessive = copy(expected);
    nudge(excessive, region.x, region.y, region.maxChannelDelta + 1);
    expect(compare(excessive, name, expected, noisePolicy).ok).toBe(false);
    for (const [x, y] of [[region.x - 1, region.y], [region.x + 1, region.y], [region.x, region.y - 1], [region.x, region.y + 1]]) {
      if (registered.has(`${x},${y}`)) continue;
      const adjacent = copy(observed); nudge(adjacent, x, y, 1);
      const result = compare(adjacent, name, expected, noisePolicy);
      expect(result.ok).toBe(false); expect(result.changedOutsideRegions).toBe(1);
    }
  }
  const shifted = copy(observed);
  for (let y = 0; y < shifted.height; y++) for (let x = shifted.width - 1; x > 0; x--) paint(shifted, x, y, pixel(observed, x - 1, y));
  expect(compare(shifted, name, expected, noisePolicy).ok).toBe(false);
  expect(compare({ ...observed, height: observed.height - 1 }, name, expected, noisePolicy).ok).toBe(false);
  expect(compare(observed, `composer-idle-draft-${theme}-390-linux.png`, expected, noisePolicy).ok).toBe(false);
});

it('the 90 fresh-context Composer measurements retain one complete fingerprint per theme', () => {
  const evidence = JSON.parse(readFileSync(new URL('../../docs/evidence/issue-441-composer-rasterizer.json', import.meta.url), 'utf8')) as {
    themes: { theme: string; fingerprintHash: string; contexts: { fingerprintHash: string; pngHash: string; rgbaHash: string; captures: number }[];
      variants: { pngHash: string; rgbaHash: string; contexts: number; pixels: MeasuredNoisePixel[] }[];
      fingerprint: { nodes: Record<string, unknown>[]; ancestors: Record<string, unknown>[] };
      computedStyleBase: Record<string, string>; computedStyleDeltas: Record<string, string>[] }[];
  };
  for (const theme of evidence.themes) {
    // Losslessly inflate the style table. Equality includes every float box,
    // DOM/attribute, focus/state value and all computed CSS/pseudo/ancestor styles.
    for (const node of [...theme.fingerprint.nodes, ...theme.fingerprint.ancestors])
      for (const key of ['style', 'before', 'after']) if (key in node) {
        expect(typeof node[key]).toBe('number');
        node[key] = { ...theme.computedStyleBase, ...theme.computedStyleDeltas[node[key] as number] };
      }
    expect(createHash('sha256').update(JSON.stringify(theme.fingerprint)).digest('hex')).toBe(theme.fingerprintHash);
    expect(theme.contexts).toHaveLength(theme.theme === 'light' ? 30 : 60);
    expect(theme.variants).toHaveLength(2);
    for (const context of theme.contexts) {
      expect(context.fingerprintHash).toBe(theme.fingerprintHash); expect(context.captures).toBeGreaterThanOrEqual(2);
      expect(theme.variants.some(v => v.pngHash === context.pngHash && v.rgbaHash === context.rgbaHash)).toBe(true);
    }
    for (const variant of theme.variants) expect(theme.contexts.filter(c => c.pngHash === variant.pngHash)).toHaveLength(variant.contexts);
    const measured = noisePolicy.find(e => e.reference === `composer-running-draft-${theme.theme}-390-linux.png`)!.pixels;
    expect(theme.variants.find(v => v.pixels.length > 0)!.pixels).toEqual(measured);
  }
});

it('D: the same low-amplitude change outside every approved region fails', () => {
  const expected = fixture(NARROW);
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
  const light = fixture(INVENTORY);
  expect(pixel(light, 400, 400)).toEqual([255, 255, 255, 255]);
  const shifted = copy(light);
  nudge(shifted, 400, 400, 7);
  const lightResult = compare(shifted, INVENTORY);
  expect(lightResult.ok).toBe(false);
  expect(lightResult.changedOutsideRegions).toBe(1);
  expect(lightResult.maxObservedDelta).toBe(7);
  // A reference with no manifest entry is strict: one changed pixel fails.
  const strict = copy(fixture(GENERAL));
  nudge(strict, 400, 400, 1);
  const strictResult = compare(strict, GENERAL);
  expect(strictResult.ok).toBe(false);
  expect(strictResult.report).toContain('no noise regions are registered');
  // Reference-locality: the inventory evidence gives an unrelated screenshot
  // zero tolerance, even at the exact approved coordinates and delta.
  const unrelated = copy(fixture(GENERAL));
  const inventoryEntry = entryOf(INVENTORY);
  for (const [x, y] of (inventoryEntry.pixels as MeasuredNoisePixel[]).map(([x, y]) => [x, y] as const))
    nudge(unrelated, x, y, inventoryEntry.regions.find(r => x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)!.maxChannelDelta);
  const unrelatedResult = compare(unrelated, GENERAL);
  expect(unrelatedResult.ok).toBe(false);
  expect(unrelatedResult.changedOutsideRegions).toBe((inventoryEntry.pixels as MeasuredNoisePixel[]).length);
});

it('E: a large-area RGB +7 regression fails (the defect the old global threshold accepted)', () => {
  const expected = fixture(NARROW);
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
  const result = compare(actual, NARROW, expected, comparatorPolicy);
  // The removed contract (per-pixel threshold 0.027, maxDiffPixels 0)
  // classified every one of these 7-level shifts as identical and passed.
  expect(result.totalChanged).toBeGreaterThanOrEqual(50_000);
  expect(result.changedOutsideRegions).toBe(result.totalChanged);
  expect(result.ok).toBe(false);
  expect(result.firstUnexpected.length).toBeLessThanOrEqual(10);
  // Failure artifacts stay bounded and mark the unexpected area red.
  const diffed = compareScreenshot({ referenceName: NARROW, expected, actual, noisePolicy: comparatorPolicy, renderDiff: true });
  expect(diffed.ok).toBe(false);
  expect(diffed.diff).toBeDefined();
  expect(diffed.diff!.width).toBe(expected.width);
  expect(pixel(diffed.diff!, 117, 564)).toEqual([255, 0, 0, 255]);
});

it('F: a whole-image low-amplitude shift fails (the defect the old global threshold accepted)', () => {
  const expected = fixture(INVENTORY);
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
  const expected = fixture(NARROW);
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
  const narrow = fixture(NARROW);
  const corner = entryOf(NARROW).regions[0]; // delta bound 3
  const overDelta = copy(narrow);
  nudge(overDelta, corner.x, corner.y, corner.maxChannelDelta + 1);
  const narrowResult = compare(overDelta, NARROW);
  expect(narrowResult.ok).toBe(false);
  expect(narrowResult.regions[0].overDeltaPixels).toBe(1);
  expect(narrowResult.report).toContain(`exceeds the approved bound ${corner.maxChannelDelta}`);
  // Even with an in-budget pixel count, an arbitrary colour change inside the
  // coordinate box is not rasterizer noise.
  const light = fixture(INVENTORY);
  const cog = entryOf(INVENTORY).regions[0]; // delta bound 7
  const lightOver = copy(light);
  nudge(lightOver, cog.x, cog.y, cog.maxChannelDelta + 1);
  const lightResult = compare(lightOver, INVENTORY);
  expect(lightResult.ok).toBe(false);
  expect(lightResult.report).toContain(`exceeds the approved bound ${cog.maxChannelDelta}`);
});

it('I: a one-pixel layout shift fails', () => {
  const expected = fixture(GENERAL);
  const actual = copy(expected);
  for (let y = 0; y < expected.height; y++)
    for (let x = expected.width - 1; x > 0; x--) paint(actual, x, y, pixel(expected, x - 1, y));
  expect(compare(actual, GENERAL).ok).toBe(false);
});

it('J: a missing element fails', () => {
  const expected = fixture(GENERAL);
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
  const expected = fixture(GENERAL);
  const narrow = fixture(NARROW);
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

it.each([['idle-empty', 'light', 12, 2, 51], ['idle-empty', 'dark', 12, 2, 51]] as const)('measured Composer corner noise stays restricted: %s %s', (state, theme, sites, bound, cornerY) => {
  const name = `composer-${state}-${theme}-390-linux.png`;
  const expected = reference(name);
  const entry = noisePolicy.find(entry => entry.reference === name)!;
  const measured = entry.pixels as MeasuredNoisePixel[];
  expect(entry.regions.flatMap(cells)).toEqual(measured.map(([x, y]) => [x, y]));
  expect(measured).toHaveLength(sites);
  const observed = copy(expected);
  for (const [x, y, before, after] of measured) {
    expect(pixel(expected, x, y)).toEqual(before);
    paint(observed, x, y, after);
  }
  expect(compare(observed, name, expected, noisePolicy).ok).toBe(true);
  expect(compare(observed, name, expected, []).ok).toBe(false);
  expect(compare(observed, `composer-idle-draft-${theme}-390-linux.png`, expected, noisePolicy).ok).toBe(false);
  for (const region of entry.regions) {
    expect(region.maxChannelDelta).toBeLessThanOrEqual(bound);
    const excessive = copy(expected);
    nudge(excessive, region.x, region.y, region.maxChannelDelta + 1);
    expect(compare(excessive, name, expected, noisePolicy).ok).toBe(false);
  }
  // Adjacent corner, text, and control pixels retain exact comparison.
  for (const [x, y] of [[34, cornerY], [33, cornerY + 2], [24, cornerY + 4], [50, 76], [300, 126]]) {
    const changed = copy(observed); nudge(changed, x, y, 1);
    const result = compare(changed, name, expected, noisePolicy);
    expect(result.ok).toBe(false); expect(result.changedOutsideRegions).toBe(1);
  }
  const shifted = copy(observed);
  for (let y = 0; y < shifted.height; y++)
    for (let x = shifted.width - 1; x > 0; x--) paint(shifted, x, y, pixel(observed, x - 1, y));
  expect(compare(shifted, name, expected, noisePolicy).ok).toBe(false);
  expect(compare({ ...observed, height: observed.height - 1 }, name, expected, noisePolicy).ok).toBe(false);
});

it.each(['light', 'dark'])('integrated running Composer stays strict outside measured sites: %s', theme => {
  const name = `composer-running-draft-${theme}-390-linux.png`;
  const entry = noisePolicy.find(entry => entry.reference === name)!;
  const expected = reference(name);
  for (const [x, y] of [[24, 117], [33, 120], [34, 117], [50, 76], [300, 126]]) {
    expect(entry.regions.some(region => cells(region).some(([rx, ry]) => rx === x && ry === y))).toBe(false);
    const changed = copy(expected); nudge(changed, x, y, 1);
    expect(compare(changed, name, expected, noisePolicy).ok).toBe(false);
  }
  expect(compare({ ...expected, height: expected.height - 1 }, name, expected, noisePolicy).ok).toBe(false);
});

const MOBILE_EXPANDED = 'mobile-expanded-dark-linux.png';
it('mobile expanded measured variants are exact-reference-local evidence', () => {
  const expected = reference(MOBILE_EXPANDED);
  const entry = noisePolicy.find(entry => entry.reference === MOBILE_EXPANDED)!;
  expect(compare(expected, MOBILE_EXPANDED, expected, noisePolicy).ok).toBe(true);
  const sites = entry.regions.flatMap(cells);
  expect(sites).toEqual(entry.pixels!.map(([x, y]) => [x, y]));
  expect(new Set(sites.map(([x, y]) => `${x},${y}`)).size).toBe(sites.length);
  for (const region of entry.regions) {
    expect([region.width, region.height, region.maxChangedPixels]).toEqual([1, 1, 1]);
  }
  // Independent glyph-edge and rounded-card variants, and their combination.
  const variants = [
    { select: (x: number) => x === 303, hash: 'c35466f1b179ca625beca2eac7202a454b38789a0265cced9a6d2d12c8c2db6e' },
    { select: (x: number) => x !== 303, hash: 'f1dc128c4037bc992ee4813e80e1b23b8c30e7e80976150157ae932b29e574c5' },
    { select: () => true, hash: 'f23070483875d43834555c8fae6848a8f874293b463bd4e47ec2007f37c378b6' },
  ];
  for (const { select, hash } of variants) {
    const actual = copy(expected);
    for (const [x, y, , after] of entry.pixels!) if (select(x)) paint(actual, x, y, after);
    expect(createHash('sha256').update(actual.data).digest('hex')).toBe(hash);
    expect(compare(actual, MOBILE_EXPANDED, expected, noisePolicy).ok).toBe(true);
    expect(compare(actual, MOBILE_EXPANDED, expected, []).ok).toBe(false);
    expect(compare(actual, 'mobile-rail-dark-linux.png', expected, noisePolicy).ok).toBe(false);
  }
});

it('mobile expanded evidence cannot conceal neighboring glyphs, gaps, contrast or layout changes', () => {
  const expected = reference(MOBILE_EXPANDED);
  const entry = noisePolicy.find(entry => entry.reference === MOBILE_EXPANDED)!;
  for (const region of entry.regions) {
    const beyond = copy(expected);
    nudge(beyond, region.x, region.y, region.maxChannelDelta + 1);
    expect(compare(beyond, MOBILE_EXPANDED, expected, noisePolicy).ok).toBe(false);
  }
  const glyphRows = new Set(entry.pixels!.filter(([x]) => x === 303).map(([, y]) => y));
  const neighbors = [...glyphRows].flatMap(y => [[302, y], [304, y]]);
  for (let y = Math.min(...glyphRows); y <= Math.max(...glyphRows); y++) {
    if (!glyphRows.has(y)) neighbors.push([303, y]);
  }
  neighbors.push([320, 250]); // A separate transcript content pixel.
  for (const [x, y] of neighbors) {
    const actual = copy(expected); nudge(actual, x, y, 1);
    expect(compare(actual, MOBILE_EXPANDED, expected, noisePolicy).ok).toBe(false);
  }
  const shifted = copy(expected);
  const theme = copy(expected);
  for (let y = 0; y < expected.height; y++) {
    for (let x = expected.width - 1; x > 0; x--) paint(shifted, x, y, pixel(expected, x - 1, y));
    for (let x = 0; x < expected.width; x++) nudge(theme, x, y, 1);
  }
  for (const actual of [shifted, theme, { ...expected, width: expected.width - 1 }, { ...expected, height: expected.height - 1 }]) {
    expect(compare(actual, MOBILE_EXPANDED, expected, noisePolicy).ok).toBe(false);
  }
});
