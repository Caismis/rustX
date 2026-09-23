/** The one screenshot comparison contract of every rustX browser reference.
 *
 * Screenshot baselines are exact product-output contracts by default: the
 * geometry must match exactly and every RGBA channel of every pixel must match
 * exactly. Known rasterizer noise is an explicit, evidence-backed exception —
 * never an inference. A difference is ignored only when the repository holds a
 * manifest entry for this exact reference naming a bounded region this pixel
 * sits in, with a changed-pixel count inside the region's budget and a raw
 * channel delta inside the region's measured bound. Everything else fails:
 * a low-amplitude difference outside a registered region, a widespread
 * low-amplitude difference, a layout change, a missing element, a theme-token
 * or contrast change, a dimension change.
 *
 * The policy lives in `test/fixtures/rasterizer-noise.json`: reference-local
 * measured evidence (raw pixel tuples plus the run that produced them) and the
 * small regions derived from it. Tolerance is evidence, not inference — nothing
 * is inferred from the screenshot under test, and one reference's evidence
 * gives an unrelated reference zero tolerance.
 *
 * `compareScreenshot` is a pure function over decoded images so the pixel
 * contract is unit-testable without launching a browser. It judges only a
 * capture already proven stable by `test/screenshot-stability.ts`; browser
 * capture, reference files and artifacts live in `test/e2e/screenshot.ts`. */
import { PNG } from 'pngjs';

/** A decoded RGBA image: `data` holds `width * height * 4` channel bytes. */
export interface RgbaImage {
  width: number;
  height: number;
  data: Uint8Array;
}

export type Rgba = [number, number, number, number];

/** One measured rasterizer-noise pixel: `[x, y, reference, observed]`. */
export type MeasuredNoisePixel = [number, number, Rgba, Rgba];

/** A bounded rectangle of approved rasterizer noise for one reference. */
export interface NoiseRegion {
  label: string;
  x: number;
  y: number;
  width: number;
  height: number;
  /** Measured budget: the region tolerates at most this many changed pixels. */
  maxChangedPixels: number;
  /** Measured bound: no tolerated pixel may exceed this absolute RGBA delta. */
  maxChannelDelta: number;
}

/** The comparison-policy manifest entry for exactly one reference image. */
export interface NoisePolicyEntry {
  /** Full snapshot file name, e.g. `settings-agent-narrow-dark-linux.png`. */
  reference: string;
  /** Where the allowance came from (CI run ids, capture provenance). */
  evidence: string;
  regions: NoiseRegion[];
  /** Raw measured changes the regions were derived from; ignored at compare time. */
  pixels?: MeasuredNoisePixel[];
}

/** The explicit exception contract. Absent entry → strict exact comparison. */
export type NoisePolicy = readonly NoisePolicyEntry[];

export interface CompareScreenshotInput {
  /** The reference file name the actual capture is being judged against. */
  referenceName: string;
  expected: RgbaImage;
  actual: RgbaImage;
  noisePolicy: NoisePolicy;
  /** Also build a diff image when the comparison fails (Playwright artifacts). */
  renderDiff?: boolean;
}

/** Per-region outcome, reported on both pass and failure. */
export interface RegionOutcome {
  label: string;
  x: number;
  y: number;
  width: number;
  height: number;
  changedPixels: number;
  maxChangedPixels: number;
  overDeltaPixels: number;
  maxObservedDelta: number;
  maxChannelDelta: number;
}

export interface ScreenshotComparison {
  ok: boolean;
  referenceName: string;
  /** Deterministic, bounded failure/pass description. Never dumps raw pixels. */
  report: string;
  expectedWidth: number;
  expectedHeight: number;
  actualWidth: number;
  actualHeight: number;
  /** Raw changed pixels across the whole image. */
  totalChanged: number;
  /** Changed pixels outside every approved region (zero tolerance by default). */
  changedOutsideRegions: number;
  /** Largest absolute RGBA channel delta observed anywhere in the image. */
  maxObservedDelta: number;
  /** First unexpected (outside-region) coordinates, row-major, capped at 10. */
  firstUnexpected: { x: number; y: number; delta: number }[];
  regions: RegionOutcome[];
  /** Failure-only marking: red = unexpected, orange = over-delta, gold = in policy. */
  diff?: RgbaImage;
}

const MAX_REPORTED_COORDINATES = 10;

/** Decode a PNG buffer into RGBA bytes (`pngjs`, a direct dev dependency). */
export function decodePng(buffer: Buffer): RgbaImage {
  const png = PNG.sync.read(buffer);
  if (png.data.length !== png.width * png.height * 4)
    throw new Error(`PNG decoded to ${png.data.length} bytes for ${png.width}x${png.height}; RGBA8 required`);
  return { width: png.width, height: png.height, data: png.data };
}

/** Encode an RGBA image as a PNG buffer (failure artifacts and diff images). */
export function encodePng(image: RgbaImage): Buffer {
  const png = new PNG({ width: image.width, height: image.height });
  png.data.set(image.data);
  return PNG.sync.write(png);
}

function channelDelta(expected: Uint8Array, actual: Uint8Array, i: number): number {
  return Math.max(
    Math.abs(expected[i] - actual[i]),
    Math.abs(expected[i + 1] - actual[i + 1]),
    Math.abs(expected[i + 2] - actual[i + 2]),
    Math.abs(expected[i + 3] - actual[i + 3]),
  );
}

function contains(region: NoiseRegion, x: number, y: number): boolean {
  return x >= region.x && x < region.x + region.width && y >= region.y && y < region.y + region.height;
}

function formatRegions(outcomes: RegionOutcome[]): string[] {
  return outcomes.map(
    outcome =>
      `  region "${outcome.label}" (${outcome.x},${outcome.y} ${outcome.width}x${outcome.height}): ` +
      `changed ${outcome.changedPixels}/${outcome.maxChangedPixels}, ` +
      `channel delta ${outcome.maxObservedDelta}/${outcome.maxChannelDelta}, ` +
      `over-delta ${outcome.overDeltaPixels}`,
  );
}

/** Exact pixel comparison plus the explicit bounded rasterizer-noise policy. */
export function compareScreenshot(input: CompareScreenshotInput): ScreenshotComparison {
  const { referenceName, expected, actual, noisePolicy, renderDiff } = input;
  const policy = noisePolicy.find(entry => entry.reference === referenceName);
  const regions = policy?.regions ?? [];
  const outcomes: RegionOutcome[] = regions.map(region => ({
    label: region.label,
    x: region.x,
    y: region.y,
    width: region.width,
    height: region.height,
    changedPixels: 0,
    maxChangedPixels: region.maxChangedPixels,
    overDeltaPixels: 0,
    maxObservedDelta: 0,
    maxChannelDelta: region.maxChannelDelta,
  }));
  const overDeltaSites: { x: number; y: number; delta: number }[] = [];

  const base = {
    referenceName,
    expectedWidth: expected.width,
    expectedHeight: expected.height,
    actualWidth: actual.width,
    actualHeight: actual.height,
    totalChanged: 0,
    changedOutsideRegions: 0,
    maxObservedDelta: 0,
    firstUnexpected: [] as { x: number; y: number; delta: number }[],
    regions: outcomes,
  };

  if (expected.width !== actual.width || expected.height !== actual.height) {
    return {
      ...base,
      ok: false,
      report: [
        `Screenshot regression: ${referenceName}`,
        `  dimensions: expected ${expected.width}x${expected.height}, actual ${actual.width}x${actual.height}`,
        `  failures:`,
        `    - geometry must match exactly; no scaling or cropping is allowed`,
      ].join('\n'),
    };
  }

  const { width, height, data: expectedData } = expected;
  const actualData = actual.data;
  const diffData = renderDiff ? Uint8Array.from(expectedData) : undefined;
  const paint = (i: number, rgba: Rgba) => {
    if (diffData) diffData.set(rgba, i);
  };
  const RED: Rgba = [255, 0, 0, 255];
  const ORANGE: Rgba = [255, 140, 0, 255];
  const GOLD: Rgba = [255, 215, 0, 255];

  let totalChanged = 0;
  let outside = 0;
  let maxObservedDelta = 0;
  const firstUnexpected: { x: number; y: number; delta: number }[] = [];

  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const i = (y * width + x) * 4;
      const delta = channelDelta(expectedData, actualData, i);
      if (delta === 0) continue;
      totalChanged++;
      if (delta > maxObservedDelta) maxObservedDelta = delta;
      let region = -1;
      for (let r = 0; r < regions.length; r++)
        if (contains(regions[r], x, y)) {
          region = r;
          break;
        }
      if (region === -1) {
        outside++;
        if (firstUnexpected.length < MAX_REPORTED_COORDINATES) firstUnexpected.push({ x, y, delta });
        paint(i, RED);
        continue;
      }
      const outcome = outcomes[region];
      outcome.changedPixels++;
      if (delta > outcome.maxObservedDelta) outcome.maxObservedDelta = delta;
      if (delta > outcome.maxChannelDelta) {
        outcome.overDeltaPixels++;
        if (overDeltaSites.length < MAX_REPORTED_COORDINATES) overDeltaSites.push({ x, y, delta });
        paint(i, ORANGE);
      } else {
        paint(i, GOLD);
      }
    }
  }

  const failures: string[] = [];
  if (outside > 0)
    failures.push(
      policy
        ? `${outside} changed pixels outside every approved noise region (any raw pixel change outside a registered region fails)`
        : `${outside} changed pixels; no noise regions are registered for this reference (strict exact match)`,
    );
  for (const outcome of outcomes) {
    if (outcome.changedPixels > outcome.maxChangedPixels)
      failures.push(
        `region "${outcome.label}": ${outcome.changedPixels} changed pixels exceed the approved budget ${outcome.maxChangedPixels}`,
      );
    if (outcome.overDeltaPixels > 0)
      failures.push(
        `region "${outcome.label}": channel delta ${outcome.maxObservedDelta} exceeds the approved bound ${outcome.maxChannelDelta} in ${outcome.overDeltaPixels} pixel(s)`,
      );
  }
  const ok = failures.length === 0;

  const header = [
    `${ok ? 'Screenshot matches' : 'Screenshot regression'}: ${referenceName}`,
    `  dimensions: expected ${expected.width}x${expected.height}, actual ${actual.width}x${actual.height}`,
    `  changed pixels: ${totalChanged} (outside approved noise regions: ${outside})`,
    policy
      ? `  noise policy: ${policy.regions.length} registered region(s) — ${policy.evidence}`
      : `  noise policy: none registered (strict exact match required)`,
    ...formatRegions(outcomes),
    `  max observed channel delta: ${maxObservedDelta}`,
  ];
  if (!ok) {
    header.push(`  failures:`);
    for (const failure of failures) header.push(`    - ${failure}`);
    if (firstUnexpected.length)
      header.push(
        `  first unexpected pixels: ${firstUnexpected.map(site => `(${site.x},${site.y}) Δ${site.delta}`).join(', ')}`,
      );
    if (overDeltaSites.length)
      header.push(
        `  over-delta pixels in approved regions: ${overDeltaSites.map(site => `(${site.x},${site.y}) Δ${site.delta}`).join(', ')}`,
      );
  }

  return {
    ...base,
    ok,
    totalChanged,
    changedOutsideRegions: outside,
    maxObservedDelta,
    firstUnexpected,
    report: header.join('\n'),
    diff: !ok && renderDiff && diffData ? { width, height, data: diffData } : undefined,
  };
}
