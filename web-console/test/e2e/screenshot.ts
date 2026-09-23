/** Browser orchestration for the rustX screenshot contract.
 *
 * `expectStableScreenshot` is the only way browser references are asserted:
 *
 *   capture → exact-RGBA stabilization → stable capture → Playwright snapshot
 *   path → exact comparator + reference-local noise policy → pass / artifacts
 *
 * Captures use `toHaveScreenshot`'s own defaults (animations disabled, caret
 * hidden, CSS scale) and its rafraf-plus-poll-interval settle. The stability
 * and comparison decisions are the pure contracts in
 * `test/screenshot-stability.ts` and `test/screenshot-comparator.ts`: two
 * consecutive captures must render identically before the baseline is read at
 * all, and the stable capture is compared exactly once — a baseline match is
 * never evidence of stability, and a stable mismatch is never retried.
 * Playwright's global perceptual threshold is never involved.
 *
 * Playwright owns snapshot path resolution (`testInfo.snapshotPath` with the
 * screenshot kind, so `<spec>.spec.ts-snapshots/<name>-<platform>.png`); as in
 * `toHaveScreenshot`, one name in one test file names one baseline. Updating
 * baselines is an explicit intentional action: `RUSTX_SCREENSHOT_UPDATE=1`, as
 * set by `pnpm test:e2e:update` over the whole browser suite. An update writes
 * only a stable capture as a strict new baseline; it never creates or widens a
 * rasterizer-noise allowance. New noise evidence is measured separately and
 * recorded in `test/fixtures/rasterizer-noise.json`.
 *
 * A mismatch attaches the stable actual and a diff PNG (red: unexpected changed
 * pixel; orange: over-delta inside an approved region; gold: in-policy changed
 * pixel); a stabilization failure attaches the last two differing captures and
 * their diff. Artifacts go to the Playwright output under `test-results/`. */
import { test, type Locator, type Page, type TestInfo } from '@playwright/test';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { basename, dirname } from 'node:path';
import { decodePng, encodePng, type NoisePolicy, type RgbaImage } from '../screenshot-comparator';
import { captureStable, verifyScreenshot, type Capture, type UnstableCapture } from '../screenshot-stability';

const noisePolicy = JSON.parse(readFileSync(new URL('../fixtures/rasterizer-noise.json', import.meta.url), 'utf8')) as NoisePolicy;
const update = process.env.RUSTX_SCREENSHOT_UPDATE === '1';
const captureOptions = { animations: 'disabled', caret: 'hide', scale: 'css' } as const;

/** Mirror the expect-path's `rafrafTimeout` settle: two animation frames plus
 * the poll-interval wait before every capture, so the capture cadence is the
 * same one the pinned authority was measured under. */
function browserCapture(target: Page | Locator): Capture {
  const page: Page = typeof (target as Locator).page === 'function' ? (target as Locator).page() : (target as Page);
  return async settleMs => {
    await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
    if (settleMs) await new Promise(resolve => setTimeout(resolve, settleMs));
    return target.screenshot(captureOptions);
  };
}

const references = new Map<string, RgbaImage>();
function decodeReference(path: string): RgbaImage {
  let decoded = references.get(path);
  if (!decoded) references.set(path, (decoded = decodePng(readFileSync(path))));
  return decoded;
}

async function attachPng(testInfo: TestInfo, name: string, png: Buffer): Promise<string> {
  const path = testInfo.outputPath(`${name}.png`);
  writeFileSync(path, png);
  await testInfo.attach(name, { path, contentType: 'image/png' });
  return path;
}

async function failUnstable(testInfo: TestInfo, stem: string, unstable: UnstableCapture): Promise<never> {
  const messages = [unstable.report];
  messages.push(`  previous capture: ${await attachPng(testInfo, `${stem}-unstable-previous`, unstable.previous)}`);
  messages.push(`  last capture: ${await attachPng(testInfo, `${stem}-unstable-last`, unstable.last)}`);
  if (unstable.diff) messages.push(`  diff capture: ${await attachPng(testInfo, `${stem}-unstable-diff`, encodePng(unstable.diff))}`);
  throw new Error(messages.join('\n'));
}

/** Assert that `target` renders exactly its checked-in reference under the
 * rustX screenshot contract: a stable capture first, then exact geometry and
 * pixels, except the reference's own registered rasterizer-noise regions. */
export async function expectStableScreenshot(target: Page | Locator, referenceName: string): Promise<void> {
  const testInfo = test.info();
  const reference = testInfo.snapshotPath(referenceName, { kind: 'screenshot' });
  // The manifest is keyed by the full reference file name, platform suffix included.
  const referenceFile = basename(reference);
  const stem = basename(referenceName, '.png');
  const capture = browserCapture(target);

  if (update || !existsSync(reference)) {
    const settled = await captureStable(capture, referenceFile);
    if (!settled.stable) return failUnstable(testInfo, stem, settled);
    if (!update) {
      const actualPath = await attachPng(testInfo, `${stem}-actual`, settled.png);
      throw new Error(
        [
          `A screenshot reference does not exist at ${reference}.`,
          `Run the explicit baseline update (pnpm test:e2e:update) to create it, then review the new reference.`,
          `  actual capture: ${actualPath}`,
        ].join('\n'),
      );
    }
    mkdirSync(dirname(reference), { recursive: true });
    writeFileSync(reference, settled.png);
    console.log(`${reference} re-generated from a stable capture (RUSTX_SCREENSHOT_UPDATE).`);
    if (noisePolicy.some(entry => entry.reference === referenceFile))
      console.log(
        `${reference} has registered rasterizer-noise evidence; a new baseline does not re-measure it — re-validate its regions against the new capture.`,
      );
    return;
  }

  const verdict = await verifyScreenshot({ capture, referenceName: referenceFile, expected: decodeReference(reference), noisePolicy });
  if (!verdict.stable) return failUnstable(testInfo, stem, verdict);
  const { comparison } = verdict;
  if (comparison.ok) return;
  const messages = [comparison.report, `  reference: ${reference}`];
  messages.push(`  actual capture: ${await attachPng(testInfo, `${stem}-actual`, verdict.png)}`);
  if (comparison.diff) messages.push(`  diff capture: ${await attachPng(testInfo, `${stem}-diff`, encodePng(comparison.diff))}`);
  throw new Error(messages.join('\n'));
}
