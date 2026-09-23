/** The rendering-stability contract of every rustX browser reference.
 *
 * A baseline is compared only against a capture proven stable by two
 * consecutive render-equivalent captures. Equality with the baseline is never
 * itself evidence of stability: a transient frame that happens to equal the
 * baseline must not stand in for the frame the UI settles on.
 *
 * Two layers stay separate:
 *
 * - runtime rendering stability — exact decoded-RGBA equality between
 *   consecutive captures of one run (`sameRendering`). No rasterizer tolerance
 *   applies here: within one run the same page renders the same bytes. Encoded
 *   PNG bytes are not the contract; two encodings of one rendering are equal.
 * - cross-run accepted rasterizer variance — the reference-local noise manifest
 *   applied by `compareScreenshot`, only to the final stable capture against the
 *   checked-in baseline, exactly once.
 *
 * Capturing is injected (`Capture`), so the decision is a pure function of the
 * capture sequence: tests hand it exact frames with no browser and no clock.
 * The settle schedule mirrors Playwright's `toHaveScreenshot` poll intervals and
 * bounds the attempt count; `test/e2e/screenshot.ts` supplies the real browser
 * capture and turns a verdict into artifacts. */
import { compareScreenshot, decodePng, type NoisePolicy, type RgbaImage, type ScreenshotComparison } from './screenshot-comparator';

/** Wait before each capture, in milliseconds: Playwright's `[0, 100, 250, 500]`
 * then 1s polls. Its length is the stabilization budget: at most this many
 * captures (about 4.85s of settle waits) before an assertion is unstable. */
export const settleSchedule: readonly number[] = [0, 100, 250, 500, 1000, 1000, 1000, 1000];

/** Capture one encoded PNG after waiting `settleMs` for rendering to settle. */
export type Capture = (settleMs: number) => Promise<Buffer>;

/** Render equivalence: the same geometry and every RGBA byte identical. */
export function sameRendering(a: RgbaImage, b: RgbaImage): boolean {
  return a.width === b.width && a.height === b.height && Buffer.compare(a.data, b.data) === 0;
}

/** Rendering never settled within the budget. Carries the last two captures,
 * which still differed, and their diff; no baseline comparison was made. */
export interface UnstableCapture {
  stable: false;
  report: string;
  captures: number;
  previous: Buffer;
  last: Buffer;
  diff?: RgbaImage;
}

export type StableCapture = { stable: true; png: Buffer; image: RgbaImage; captures: number } | UnstableCapture;

/** Describe why rendering never settled: bounded, no raw pixels. */
function unstableReport(referenceName: string, captures: number, schedule: readonly number[], lastPair: ScreenshotComparison): string {
  const pair =
    lastPair.expectedWidth !== lastPair.actualWidth || lastPair.expectedHeight !== lastPair.actualHeight
      ? `${lastPair.expectedWidth}x${lastPair.expectedHeight} then ${lastPair.actualWidth}x${lastPair.actualHeight}`
      : `${lastPair.actualWidth}x${lastPair.actualHeight}, ${lastPair.totalChanged} changed pixels (max channel delta ${lastPair.maxObservedDelta})`;
  return [
    `Screenshot did not stabilize: ${referenceName}`,
    `  no two consecutive captures rendered identically (exact RGBA) in ${captures} captures`,
    `  settle waits: ${schedule.join(', ')} ms`,
    `  last two captures: ${pair}`,
    `  the baseline was not compared; an unsettled frame is never judged against it`,
  ].join('\n');
}

/** Capture until two consecutive captures render identically, within the
 * bounded schedule. The stable capture is the second of that pair. */
export async function captureStable(
  capture: Capture,
  referenceName: string,
  schedule: readonly number[] = settleSchedule,
): Promise<StableCapture> {
  if (schedule.length < 2) throw new Error(`A stability schedule needs at least two captures, got ${schedule.length}`);
  let previous: { png: Buffer; image: RgbaImage } | undefined;
  let beforePrevious: { png: Buffer; image: RgbaImage } | undefined;
  let captures = 0;
  for (const settleMs of schedule) {
    const png = await capture(settleMs);
    captures++;
    const image = decodePng(png);
    if (previous && sameRendering(previous.image, image)) return { stable: true, png, image, captures };
    beforePrevious = previous;
    previous = { png, image };
  }
  const lastPair = compareScreenshot({
    referenceName,
    expected: beforePrevious!.image,
    actual: previous!.image,
    noisePolicy: [],
    renderDiff: true,
  });
  return {
    stable: false,
    report: unstableReport(referenceName, captures, schedule, lastPair),
    captures,
    previous: beforePrevious!.png,
    last: previous!.png,
    diff: lastPair.diff,
  };
}

export type ScreenshotVerdict =
  | UnstableCapture
  /** The stable capture, compared against the baseline exactly once. */
  | { stable: true; png: Buffer; captures: number; comparison: ScreenshotComparison };

/** Stabilize, then judge the stable capture against `expected` exactly once
 * under the exact comparator and the reference's registered noise policy. */
export async function verifyScreenshot(input: {
  capture: Capture;
  referenceName: string;
  expected: RgbaImage;
  noisePolicy: NoisePolicy;
  schedule?: readonly number[];
}): Promise<ScreenshotVerdict> {
  const { capture, referenceName, expected, noisePolicy, schedule } = input;
  const settled = await captureStable(capture, referenceName, schedule);
  if (!settled.stable) return settled;
  const comparison = compareScreenshot({ referenceName, expected, actual: settled.image, noisePolicy, renderDiff: true });
  return { stable: true, png: settled.png, captures: settled.captures, comparison };
}
