const fixtureOrigin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
import { test, expect } from '@playwright/test';

for (const width of [1440, 390]) {
  test(`native phase coordinates survive browser layout at ${width}px`, async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.setViewportSize({ width, height: 700 });
    await page.goto(`${fixtureOrigin}/test/fixtures/trajectory-timing.html`);
    const measured = page.getByRole('region', { name: 'Measured bridge' })
      .getByRole('button', { name: 'Inspect Request · historical-model · request-0' });
    await expect(measured).toBeVisible();
    const geometry = await measured.evaluate(el => {
      const css = getComputedStyle(el);
      return {
        width: el.getBoundingClientRect().width,
        track: el.parentElement!.getBoundingClientRect().width,
        dispatch: css.getPropertyValue('--trajectory-dispatch'),
        first: css.getPropertyValue('--trajectory-first-output'),
        gradient: css.backgroundImage,
        minimum: css.minWidth,
      };
    });
    expect(geometry.dispatch.trim()).toBe('20%');
    expect(geometry.first.trim()).toBe('36%');
    expect(geometry.gradient).toContain('linear-gradient');
    expect(geometry.minimum).toBe('0px');
    // The 9-second Journal/reference domain cannot stretch the 2-second request.
    expect(Math.abs(geometry.width - geometry.track * 2000 / 9000)).toBeLessThan(0.1);
    expect(Math.abs(geometry.width * 0.36 - geometry.track * 720 / 9000)).toBeLessThan(0.1);
    const missing = page.getByRole('region', { name: 'Missing bridge' })
      .getByRole('button', { name: 'Inspect Request · historical-model · request-0' });
    await expect(missing).toBeVisible();
    expect(await missing.evaluate(el => getComputedStyle(el).backgroundImage)).toBe('none');
    expect(await missing.evaluate(el => el.style.getPropertyValue('--trajectory-first-output'))).toBe('');
    await page.screenshot({ path: `/tmp/rustx-364-generation-timing-${width}.png` });
    expect(errors).toEqual([]);
  });
}

test('equal-width model glyph retains measured waiting/generation ratio without inventing timed positions', async ({ page }) => {
  await page.goto(`${fixtureOrigin}/test/fixtures/trajectory-timing.html`);
  const region = page.getByRole('region', { name: 'Sequence phases' });
  const request = region.locator('[data-kind=request]');
  const tool = region.locator('[data-kind=tool]');
  await expect(request).toBeVisible();
  expect(await request.evaluate(el => el.style.getPropertyValue('--trajectory-sequence-ttft'))).toBe('20%');
  expect(await request.evaluate(el => el.style.getPropertyValue('--trajectory-first-output'))).toBe('');
  expect(await request.evaluate(el => getComputedStyle(el).backgroundImage)).toContain('linear-gradient');
  expect((await request.boundingBox())!.width).toBeCloseTo((await tool.boundingBox())!.width, 1);
  await page.screenshot({ path: '/tmp/rustx-nav-qa/trajectory-phases.png' });
});

test('pointer selection keeps exact edges, dims spans, and double-click restores overview', async ({ page }) => {
  await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html`);
  const canvas = page.getByLabel('Timeline navigation: arrow keys pan, Escape clears focus');
  const rect = (await canvas.boundingBox())!;
  const x = (fraction: number) => rect.x + rect.width * fraction;
  const y = rect.y + rect.height - 2;
  await page.mouse.move(x(.12), y);
  await page.mouse.down();
  await expect(page.locator('[data-focus-range]')).toBeVisible();
  await page.mouse.move(x(.31), y, { steps: 5 });
  await expect(canvas.locator('[data-dimmed]')).not.toHaveCount(0);
  const before = (await page.locator('[data-focus-range]').boundingBox())!;
  await page.screenshot({ path: '/tmp/rustx-nav-qa/trajectory-selection.png' });
  await page.mouse.up();
  const after = (await page.locator('[data-focus-range]').boundingBox())!;
  expect(after.x).toBeCloseTo(before.x, 1);
  expect(after.width).toBeCloseTo(before.width, 1);
  await page.mouse.dblclick(x(.6), y);
  await expect(page.locator('[data-focus-range]')).toHaveCount(0);
  await expect(canvas.locator('[data-dimmed]')).toHaveCount(0);
  await page.mouse.move(x(.5), y);
  await page.mouse.wheel(0, -100);
  await expect.poll(async () => Number(await canvas.getAttribute('data-domain-start'))).toBeGreaterThan(0);
  const start = Number(await canvas.getAttribute('data-domain-start'));
  await page.mouse.down();
  await page.mouse.move(x(.99), y, { steps: 8 });
  await expect.poll(async () => Number(await canvas.getAttribute('data-domain-start'))).toBeGreaterThan(start);
  await page.mouse.up();
  await page.mouse.click(x(.5), y, { button: 'right' });
  await expect(page.locator('[data-focus-range]')).toHaveCount(0);
});
