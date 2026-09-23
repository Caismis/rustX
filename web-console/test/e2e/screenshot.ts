/** Capture and orchestration for the rustX screenshot contract.
 *
 * `expectStableScreenshot` is the only way browser references are asserted.
 * It captures with `toHaveScreenshot`'s own defaults (animations disabled,
 * caret hidden, CSS scale), waits for rendering to settle exactly the way the
 * old matcher did — a fast-path compare on the first capture, then two
 * consecutive identical captures — and hands the bytes to the pure
 * `compareScreenshot` policy in `test/screenshot-comparator.ts`. Playwright's
 * global perceptual threshold is never involved: the comparison is the exact
 * pixel contract, it runs at most twice for one assertion, and a mismatching
 * comparison is never retried until something passes.
 *
 * Baselines live next to their spec as `<spec>.spec.ts-snapshots/<name>-<platform>.png`
 * (the layout Playwright's legacy snapshot template writes). Updating them is
 * an explicit intentional action: `RUSTX_SCREENSHOT_UPDATE=1`, as set by
 * `pnpm test:e2e:update`. An update writes a strict new baseline; it never
 * creates or widens a rasterizer-noise allowance. New noise evidence is
 * measured separately and recorded in `test/fixtures/rasterizer-noise.json`.
 *
 * Failures attach the captured actual and a diff PNG (red: unexpected changed
 * pixel; orange: over-delta inside an approved region; gold: in-policy changed
 * pixel) to the Playwright artifacts under `test-results/`. */
import { test, type Locator, type Page, type TestInfo } from '@playwright/test';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import {
  compareScreenshot,
  decodePng,
  encodePng,
  type NoisePolicy,
  type RgbaImage,
  type ScreenshotComparison,
} from '../screenshot-comparator';

const noisePolicy = JSON.parse(readFileSync(new URL('../fixtures/rasterizer-noise.json', import.meta.url), 'utf8')) as NoisePolicy;
const update = process.env.RUSTX_SCREENSHOT_UPDATE === '1';
/** Playwright's default expect timeout: the stability-wait budget per assertion. */
const stabilityTimeout = 5000;
const captureOptions = { animations: 'disabled', caret: 'hide', scale: 'css' } as const;

/** Mirror the expect-path's `rafrafTimeout` settle: two animation frames plus
 * the poll-interval wait before every capture, so the capture cadence is the
 * same one the pinned authority was measured under. */
async function capture(target: Page | Locator, preTimeout: number): Promise<Buffer> {
  const page: Page = typeof (target as Locator).page === 'function' ? (target as Locator).page() : (target as Page);
  await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
  if (preTimeout) await new Promise(resolve => setTimeout(resolve, preTimeout));
  return target.screenshot(captureOptions);
}

const references = new Map<string, RgbaImage>();
function decodeReference(path: string): RgbaImage {
  let decoded = references.get(path);
  if (!decoded) references.set(path, (decoded = decodePng(readFileSync(path))));
  return decoded;
}

/** Same-name reuse inside one test walks the `-1`, `-2`, … snapshot suffixes,
 * exactly like Playwright's per-test named snapshot index, and the reference
 * carries the platform suffix Playwright's legacy snapshot template writes. */
const nameIndexes = new WeakMap<TestInfo, Map<string, number>>();
function referencePath(testInfo: TestInfo, referenceName: string): string {
  let indexes = nameIndexes.get(testInfo);
  if (!indexes) nameIndexes.set(testInfo, (indexes = new Map()));
  const index = (indexes.get(referenceName) ?? 0) + 1;
  indexes.set(referenceName, index);
  const stem = referenceName.replace(/\.[^.]+$/, '');
  const indexed = index > 1 ? `${stem}-${index - 1}` : stem;
  return join(dirname(testInfo.file), `${basename(testInfo.file)}-snapshots`, `${indexed}-${process.platform}.png`);
}

/** Playwright's settle loop: wait for two consecutive identical captures.
 * `previous` is the capture already taken, so a stable pair may close fast. */
async function stableCapture(target: Page | Locator, previous: Buffer, referenceName: string): Promise<Buffer> {
  const deadline = Date.now() + stabilityTimeout;
  let before = previous;
  for (const preTimeout of [100, 250, 500, 1000, 1000, 1000, 1000, 1000]) {
    const remaining = deadline - Date.now();
    if (remaining <= 0) break;
    const current = await capture(target, Math.min(preTimeout, remaining));
    if (current.equals(before)) return current;
    before = current;
  }
  throw new Error(
    `Screenshot regression: ${referenceName}\n  failed to take two consecutive identical captures within ${stabilityTimeout}ms`,
  );
}

async function fail(
  testInfo: TestInfo,
  referenceName: string,
  reference: string,
  actual: Buffer,
  comparison: ScreenshotComparison,
): Promise<never> {
  const stem = basename(referenceName, '.png');
  const actualPath = testInfo.outputPath(`${stem}-actual.png`);
  writeFileSync(actualPath, actual);
  const messages = [comparison.report, `  reference: ${reference}`, `  actual capture: ${actualPath}`];
  await testInfo.attach(`${stem}-actual`, { path: actualPath, contentType: 'image/png' });
  if (comparison.diff) {
    const diffPath = testInfo.outputPath(`${stem}-diff.png`);
    writeFileSync(diffPath, encodePng(comparison.diff));
    messages.push(`  diff capture: ${diffPath}`);
    await testInfo.attach(`${stem}-diff`, { path: diffPath, contentType: 'image/png' });
  }
  throw new Error(messages.join('\n'));
}

/** Assert that `target` renders exactly its checked-in reference under the
 * rustX screenshot contract: exact geometry and pixels by default, except the
 * reference's own registered rasterizer-noise regions and their bounds. */
export async function expectStableScreenshot(target: Page | Locator, referenceName: string): Promise<void> {
  const testInfo = test.info();
  const reference = referencePath(testInfo, referenceName);
  const first = await capture(target, 0);

  if (update) {
    const stable = await stableCapture(target, first, referenceName);
    mkdirSync(dirname(reference), { recursive: true });
    writeFileSync(reference, stable);
    console.log(`${reference} re-generated, writing actual (RUSTX_SCREENSHOT_UPDATE).`);
    const policy = noisePolicy.find(entry => entry.reference === referenceName);
    if (policy)
      console.log(
        `${reference} has registered rasterizer-noise evidence; a new baseline does not re-measure it — re-validate its regions against the new capture.`,
      );
    return;
  }

  const compare = (buffer: Buffer): ScreenshotComparison =>
    compareScreenshot({
      // The manifest is keyed by the full reference file name, platform suffix included.
      referenceName: basename(reference),
      expected: decodeReference(reference),
      actual: decodePng(buffer),
      noisePolicy,
      renderDiff: true,
    });

  if (!existsSync(reference)) {
    const stem = basename(referenceName, '.png');
    const actualPath = testInfo.outputPath(`${stem}-actual.png`);
    writeFileSync(actualPath, first);
    await testInfo.attach(`${stem}-actual`, { path: actualPath, contentType: 'image/png' });
    throw new Error(
      [
        `A screenshot reference does not exist at ${reference}.`,
        `Run the explicit baseline update (pnpm test:e2e:update) to create it, then review the new reference.`,
        `  actual capture: ${actualPath}`,
      ].join('\n'),
    );
  }

  const firstAttempt = compare(first);
  if (firstAttempt.ok) return;
  // Rendering may still be settling; compare the settled capture exactly once
  // more, then fail.
  const stable = await stableCapture(target, first, referenceName);
  const finalAttempt = compare(stable);
  if (finalAttempt.ok) return;
  await fail(testInfo, referenceName, reference, stable, finalAttempt);
}
