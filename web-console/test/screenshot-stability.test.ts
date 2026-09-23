// @vitest-environment node
import { readFileSync } from 'node:fs';
import { PNG } from 'pngjs';
import { describe, expect, it } from 'vitest';
import { decodePng, encodePng, type NoisePolicy, type RgbaImage } from './screenshot-comparator';
import { captureStable, sameRendering, settleSchedule, verifyScreenshot, type Capture } from './screenshot-stability';

/** The rendering-stability contract, held to exact capture sequences.
 *
 * A capture sequence stands in for the browser: each frame is an exact image,
 * no browser, no sleeps and no clock are involved. These tests prove that the
 * baseline is judged only after two consecutive render-equivalent captures,
 * exactly once, that equality with the baseline never short-circuits the
 * stability proof, that stability is exact decoded RGBA (not PNG bytes, and
 * never the rasterizer-noise policy), and that an unsettled rendering is its
 * own failure class, distinct from a stable mismatch. */

const REFERENCE = 'frame-linux.png';
const WIDTH = 6;
const HEIGHT = 4;

const solid = (rgba: [number, number, number, number]): RgbaImage => {
  const data = new Uint8Array(WIDTH * HEIGHT * 4);
  for (let i = 0; i < data.length; i += 4) data.set(rgba, i);
  return { width: WIDTH, height: HEIGHT, data };
};
const withPixel = (image: RgbaImage, x: number, y: number, rgba: [number, number, number, number]): RgbaImage => {
  const data = Uint8Array.from(image.data);
  data.set(rgba, (y * image.width + x) * 4);
  return { width: image.width, height: image.height, data };
};

const baseline = solid([240, 240, 240, 255]);
/** A one-pixel product change far from any noise region. */
const changed = withPixel(baseline, 5, 3, [30, 30, 30, 255]);
const transient = solid([10, 10, 10, 255]);
/** Registered rasterizer noise: a 2x2 corner region, budget 4, delta bound 3. */
const noisePolicy: NoisePolicy = [
  {
    reference: REFERENCE,
    evidence: 'synthetic stability-contract fixture',
    regions: [{ label: 'corner', x: 0, y: 0, width: 2, height: 2, maxChangedPixels: 4, maxChannelDelta: 3 }],
  },
];
const noisyA = withPixel(baseline, 0, 0, [242, 242, 242, 255]);
const noisyB = withPixel(baseline, 1, 1, [238, 238, 238, 255]);

/** Replay exact frames; record every settle wait the stabilizer asked for. */
function sequence(frames: RgbaImage[]) {
  const pngs = frames.map(encodePng);
  const settles: number[] = [];
  const capture: Capture = async settleMs => {
    if (settles.length >= pngs.length) throw new Error(`capture ${settles.length + 1} was not scripted`);
    settles.push(settleMs);
    return pngs[settles.length - 1];
  };
  return { capture, settles };
}

/** A rendering that never repeats: every capture is a new frame. */
function everChanging() {
  const settles: number[] = [];
  const capture: Capture = async settleMs => {
    settles.push(settleMs);
    return encodePng(withPixel(baseline, 2, 2, [settles.length, 0, 0, 255]));
  };
  return { capture, settles };
}

const verify = (capture: Capture) => verifyScreenshot({ capture, referenceName: REFERENCE, expected: baseline, noisePolicy });
const decoded = (png: Buffer) => decodePng(png);

describe('verifyScreenshot: the baseline is judged only after stability', () => {
  it('A: a first frame equal to the baseline does not pass when rendering settles on a changed frame', async () => {
    const { capture, settles } = sequence([baseline, changed, changed]);
    const verdict = await verify(capture);
    if (!verdict.stable) throw new Error(verdict.report);
    expect(settles).toHaveLength(3);
    expect(verdict.captures).toBe(3);
    expect(sameRendering(decoded(verdict.png), changed)).toBe(true);
    expect(verdict.comparison.ok).toBe(false);
    expect(verdict.comparison.totalChanged).toBe(1);
    expect(verdict.comparison.firstUnexpected).toEqual([{ x: 5, y: 3, delta: 210 }]);
  });

  it('B: a transient first frame is not judged; the settled baseline passes', async () => {
    const { capture, settles } = sequence([transient, baseline, baseline]);
    const verdict = await verify(capture);
    if (!verdict.stable) throw new Error(verdict.report);
    expect(settles).toHaveLength(3);
    expect(sameRendering(decoded(verdict.png), baseline)).toBe(true);
    expect(verdict.comparison.ok).toBe(true);
  });

  it('C: an already stable baseline still takes two captures before it passes', async () => {
    const { capture, settles } = sequence([baseline, baseline]);
    const verdict = await verify(capture);
    if (!verdict.stable) throw new Error(verdict.report);
    expect(settles).toEqual(settleSchedule.slice(0, 2));
    expect(verdict.captures).toBe(2);
    expect(verdict.comparison.ok).toBe(true);
  });

  it('D: a stable regression fails at once and is never recaptured hoping for a match', async () => {
    // A third frame equal to the baseline is scripted but must never be taken.
    const { capture, settles } = sequence([changed, changed, baseline]);
    const verdict = await verify(capture);
    if (!verdict.stable) throw new Error(verdict.report);
    expect(settles).toHaveLength(2);
    expect(verdict.comparison.ok).toBe(false);
    expect(verdict.comparison.report).toContain('Screenshot regression: frame-linux.png');
  });

  it('E: several transitions converge on the first consecutive identical pair', async () => {
    const a = solid([1, 2, 3, 255]);
    const b = solid([4, 5, 6, 255]);
    const { capture, settles } = sequence([a, b, changed, changed]);
    const verdict = await verify(capture);
    if (!verdict.stable) throw new Error(verdict.report);
    expect(settles).toEqual(settleSchedule.slice(0, 4));
    expect(sameRendering(decoded(verdict.png), changed)).toBe(true);
    expect(verdict.comparison.ok).toBe(false);
  });

  it('F: a rendering that never settles is a stabilization failure, never a comparison', async () => {
    const { capture, settles } = everChanging();
    const verdict = await verify(capture);
    expect(verdict.stable).toBe(false);
    if (verdict.stable) return;
    expect(settles).toEqual([...settleSchedule]);
    expect(verdict.captures).toBe(settleSchedule.length);
    expect('comparison' in verdict).toBe(false);
    expect(verdict.report).toBe(
      [
        'Screenshot did not stabilize: frame-linux.png',
        `  no two consecutive captures rendered identically (exact RGBA) in ${settleSchedule.length} captures`,
        `  settle waits: ${settleSchedule.join(', ')} ms`,
        '  last two captures: 6x4, 1 changed pixels (max channel delta 1)',
        '  the baseline was not compared; an unsettled frame is never judged against it',
      ].join('\n'),
    );
    // The last two differing captures and their diff are kept for artifacts.
    expect(decoded(verdict.previous).data[(2 * WIDTH + 2) * 4]).toBe(settleSchedule.length - 1);
    expect(decoded(verdict.last).data[(2 * WIDTH + 2) * 4]).toBe(settleSchedule.length);
    expect(verdict.diff).toBeDefined();
  });

  it('F: a baseline frame seen again and again between changes never passes without a stable pair', async () => {
    const frames = settleSchedule.map((_, index) => (index % 2 ? changed : baseline));
    const { capture } = sequence(frames);
    const verdict = await verify(capture);
    expect(verdict.stable).toBe(false);
  });
});

describe('stability is exact decoded RGBA', () => {
  it('G: different PNG encodings of one rendering are render-equivalent and stable', async () => {
    const encode = (deflateLevel: number) => {
      const png = new PNG({ width: WIDTH, height: HEIGHT });
      png.data.set(changed.data);
      return PNG.sync.write(png, { deflateLevel, filterType: deflateLevel ? 4 : 0 });
    };
    const stored = encode(0);
    const compressed = encode(9);
    expect(stored.equals(compressed)).toBe(false);
    expect(sameRendering(decodePng(stored), decodePng(compressed))).toBe(true);
    const frames = [stored, compressed];
    const settled = await captureStable(async () => frames.shift()!, REFERENCE);
    expect(settled.stable).toBe(true);
    expect(settled.captures).toBe(2);
  });

  it('G: any single channel byte, alpha included, or any geometry change is a different rendering', () => {
    expect(sameRendering(baseline, solid([240, 240, 240, 255]))).toBe(true);
    for (const rgba of [
      [241, 240, 240, 255],
      [240, 241, 240, 255],
      [240, 240, 241, 255],
      [240, 240, 240, 254],
    ] as const)
      expect(sameRendering(baseline, withPixel(baseline, 3, 1, [...rgba]))).toBe(false);
    // Same bytes, transposed geometry.
    const wide: RgbaImage = { width: 2, height: 1, data: new Uint8Array(8) };
    const tall: RgbaImage = { width: 1, height: 2, data: new Uint8Array(8) };
    expect(sameRendering(wide, tall)).toBe(false);
  });

  it('the rasterizer-noise policy never makes two different frames stable', async () => {
    // Each noisy frame alone passes the baseline policy…
    for (const noisy of [noisyA, noisyB]) {
      const verdict = await verify(sequence([noisy, noisy]).capture);
      if (!verdict.stable) throw new Error(verdict.report);
      expect(verdict.comparison.ok).toBe(true);
      expect(verdict.comparison.totalChanged).toBe(1);
    }
    // …but alternating in-policy frames are still an unsettled rendering.
    const frames = settleSchedule.map((_, index) => (index % 2 ? noisyA : noisyB));
    const verdict = await verify(sequence(frames).capture);
    expect(verdict.stable).toBe(false);
  });

  it('noise that settles is judged on its stable frame under the policy', async () => {
    const { capture } = sequence([noisyA, noisyB, noisyB]);
    const verdict = await verify(capture);
    if (!verdict.stable) throw new Error(verdict.report);
    expect(sameRendering(decoded(verdict.png), noisyB)).toBe(true);
    expect(verdict.comparison.ok).toBe(true);
  });
});

describe('baseline update coverage', () => {
  it('the update command is the whole browser suite, so every expectStableScreenshot is reachable', () => {
    const { scripts } = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')) as {
      scripts: Record<string, string>;
    };
    // No spec list to drift: update mode runs exactly what `test:e2e` runs,
    // and `test:e2e` passes no spec filter to the pinned-browser wrapper.
    expect(scripts['test:e2e:update']).toBe('RUSTX_SCREENSHOT_UPDATE=1 pnpm test:e2e');
    expect(scripts['test:e2e']).toBe('pnpm build && bash scripts/browser-tests.sh');
  });
});
