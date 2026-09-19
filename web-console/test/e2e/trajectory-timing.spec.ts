import { test, expect } from '@playwright/test';

for (const width of [1440, 390]) {
  test(`native phase coordinates survive browser layout at ${width}px`, async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.setViewportSize({ width, height: 700 });
    await page.goto('http://127.0.0.1:5174/test/fixtures/trajectory-timing.html');
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
