import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { choose, connectRemote, openEmptySession } from './shell-actions';
import { wireProbe } from './wire-probe';

for (const [width, locale, theme] of [[1440, 'en', 'light'], [390, 'zh', 'dark']] as const) {
  test(`DSH Stop preserves interrupted output and disclosures ${width} ${locale} ${theme}`, async ({ page }) => {
    page.setDefaultTimeout(15000);
    const fixture = await startDogfood('web_stop');
    const wire = await wireProbe(page);
    let passed = false;
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    const zh = locale === 'zh';
    const localize = async () => {
      if (!zh) return;
      await page.getByRole('button', { name: 'Settings', exact: true }).click();
      await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文');
      await page.getByRole('button', { name: '关闭设置' }).click();
    };
    const message = page.getByRole('textbox', { name: zh ? '消息' : 'Message', exact: true });
    const controls = page.locator('[data-turn-process]').filter({ hasText: zh ? /^已停止$/ : /^Stopped$/ });
    const markers = page.locator('[data-assistant-stopped]');
    const stop = async (count: number) => {
      await page.getByRole('button', { name: zh ? '停止' : 'Stop', exact: true }).click();
      await expect(controls).toHaveCount(count);
      await expect(controls.last()).toBeDisabled();
      await expect(page.locator('[data-chat-running]')).toHaveCount(0);
      await expect(page.getByRole('button', { name: zh ? '发送' : 'Send', exact: true })).toBeVisible();
    };
    const send = async (text: string) => { await message.fill(text); await page.getByRole('button', { name: zh ? '发送' : 'Send', exact: true }).click(); };
    try {
      await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await page.addInitScript(theme => localStorage.setItem('rustx-appearance-v1', theme), theme);
      await routeWorkspaceHost(page, fixture); await page.goto('/');
      await expect(page).toHaveTitle(/rustX/);
      await connectRemote(page, fixture.endpoint, fixture.token);
      await openEmptySession(page, fixture, 'Workspace A'); await localize();
      await send('Stop before output'); await fixture.gate('stop-empty'); await stop(1);
      await fixture.release('stop-empty');
      await expect(markers).toHaveCount(0);
      await expect(page.getByText('Abandoned empty response.', { exact: true })).toHaveCount(0);

      await send('Stop during reasoning'); await fixture.gate('stop-reasoning');
      const reasoning = page.getByRole('button', { name: zh ? /^思考/ : /^Reasoning/ });
      await reasoning.click();
      const reasoningElement = await reasoning.elementHandle();
      await stop(2); await fixture.release('stop-reasoning');
      expect(await reasoningElement!.evaluate(element => element.isConnected && element.getAttribute('aria-expanded') === 'true')).toBe(true);
      await expect(markers).toHaveCount(1); await expect(markers.last()).toBeVisible();
      await expect(page.getByText('Preserved reasoning before Stop.', { exact: true })).toBeVisible();

      await send('Stop during text'); await fixture.gate('stop-text');
      const heading = page.getByRole('heading', { name: 'Preserved partial answer' });
      await expect(heading).toBeVisible(); const headingElement = await heading.elementHandle();
      await stop(3); await fixture.release('stop-text');
      expect(await headingElement!.evaluate(element => element.isConnected)).toBe(true);
      await expect(heading).toHaveCount(1); await expect(markers).toHaveCount(2);
      const markerStyle = await markers.last().evaluate(element => ({ width: element.getBoundingClientRect().width, height: element.getBoundingClientRect().height, gap: getComputedStyle(element).marginTop }));
      expect(markerStyle.width).toBeLessThan(100); expect(markerStyle.height).toBe(18); expect(markerStyle.gap).toBe('16px');
      await expect(page.getByText('fn unfinished() {', { exact: true })).toBeVisible();
      await expect(page.locator('[data-turn-tail]')).toHaveCount(0);
      await page.screenshot({ path: `/tmp/rustx-stop-text-${width}.png` });

      await send('Stop during long text'); await fixture.gate('stop-long');
      const longHeading = page.getByRole('heading', { name: 'Long partial answer' });
      await expect(longHeading).toBeVisible(); await expect(page.getByText('Preserved long tail.', { exact: true })).toBeVisible();
      const longElement = await longHeading.elementHandle();
      await stop(4); await fixture.release('stop-long');
      await expect(longHeading).toBeVisible(); await expect(page.getByText('Preserved long tail.', { exact: true })).toBeVisible();
      expect(await longElement!.evaluate(element => element.isConnected)).toBe(true);
      await expect(markers).toHaveCount(3);

      await send('Stop during proposal'); await fixture.gate('stop-proposal');
      await stop(5); await fixture.release('stop-proposal');
      await expect(markers).toHaveCount(4); await expect(markers.last()).toHaveText(zh ? '已停止' : 'Stopped');
      await expect(page.getByText(/Assembling|组装中/)).toHaveCount(0);
      await expect(page.locator('[data-tool-call-id="stopped-write"]')).toHaveCount(0);
      await expect(page.getByText('Preparing a file without executing it.', { exact: true })).toBeVisible();

      await send('Stop running tool');
      await page.getByRole('button', { name: zh ? '允许一次' : 'Allow once', exact: true }).click();
      await expect(page.locator('[data-tool-call-id="stopped-bash"] > [data-state]')).toHaveAttribute('data-state', 'running');
      await stop(6);
      await expect(page.getByRole('button', { name: zh ? '允许一次' : 'Allow once', exact: true })).toHaveCount(0);
      await expect(page.locator('[data-tool-call-id="stopped-bash"] > [data-state]')).toHaveAttribute('data-state', 'stopped');
      await expect(markers).toHaveCount(4);
      await send('Continue after all stops');
      await expect(page.getByText('The conversation continued after all stops.', { exact: true })).toBeVisible();
      await expect(page.getByText(/Abandoned .* response\./)).toHaveCount(0);
      await expect(controls).toHaveCount(6); await expect(markers).toHaveCount(4);

      // Credentials are memory-only; restore the real attachment after reload.
      await page.evaluate(() => localStorage.setItem('rustx-locale-v1', 'en'));
      const beforeRestore = wire.responses.length;
      await page.reload(); await connectRemote(page, fixture.endpoint, fixture.token);
      // Changing language opens Settings, which intentionally retires pending
      // Session preparation. Wait for this restored attachment's subscription.
      await expect.poll(() => wire.responses.slice(beforeRestore).some(row => row.method === 'session/subscribe')).toBe(true);
      await expect(page.getByText('The conversation continued after all stops.', { exact: true })).toBeVisible();
      await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
      await localize();
      // Reload starts from the native bounded latest page. Older stopped
      // output is recovered through the ordinary history gesture.
      const earlier = page.getByRole('button', { name: zh ? '加载更早' : 'Load earlier', exact: true });
      await expect.poll(async () => await controls.count() === 6 || await earlier.isVisible()).toBe(true);
      if (await earlier.isVisible()) {
        try { await earlier.click({ timeout: 2000 }); }
        catch (error) { if (await controls.count() !== 6) throw error; }
      }
      await expect(controls).toHaveCount(6); await expect(markers).toHaveCount(4);
      await expect(heading).toHaveCount(1);
      await expect(longHeading).toHaveCount(1); await expect(page.getByText('Preserved long tail.', { exact: true })).toBeVisible();
      await expect(page.getByText(/Abandoned .* response\./)).toHaveCount(0);
      await expect(page.locator('[data-chat-running]')).toHaveCount(0);
      await expect(page.locator('vite-error-overlay')).toHaveCount(0);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      if (await page.locator('[data-chat-latest]').isVisible()) await page.locator('[data-chat-latest]').click();
      await page.screenshot({ path: `/tmp/rustx-stop-history-${width}.png` });
      expect(errors).toEqual([]); passed = true;
    } catch (error) {
      await test.info().attach('native-stop-observations', { body: JSON.stringify({ errors, responses: wire.responses.slice(-12), notifications: wire.notifications.slice(-12) }), contentType: 'application/json' });
      throw error;
    } finally { await fixture.stop(passed); }
  });
}
