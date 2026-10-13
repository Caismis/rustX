import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openEmptySession, showInspector } from './shell-actions';
import { wireProbe } from './wire-probe';

for (const width of [1440, 390]) for (const acknowledgement of ['received', 'lost'] as const) {
  test(`live same-model profile selection (${width}, acknowledgement ${acknowledgement})`, async ({ page }) => {
    // Reuse the shared two-request provider scenario. Its first stream is held
    // at an explicit gate until the profile mutation and recovery are proven.
    const fixture = await startDogfood('tui_multi_session');
    let passed = false;
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => {
      if (message.type() === 'error' || message.type() === 'warning') errors.push(message.text());
    });
    const wire = await wireProbe(page);
    try {
      await page.setViewportSize({ width, height: 1000 });
      await routeWorkspaceHost(page, fixture);
      await page.goto('/');
      await expect(page).toHaveTitle(/rustX/);
      await connectRemote(page, fixture.endpoint, fixture.token);
      const sessionId = await openEmptySession(page, fixture, 'Workspace A');
      const collapse = page.getByRole('button', { name: 'Collapse Sidebar', exact: true });
      if (width < 600 && await collapse.isVisible()) await collapse.click();
      const input = page.getByRole('textbox', { name: 'Message', exact: true });
      await input.fill('tui multi-session: session A long task');
      await input.press('Enter');
      await fixture.gate('session-a-holding');
      const running = page.locator('[data-running-model]');
      await expect(running).toHaveText('Running: fixture/integration-model / high');
      const trigger = page.getByRole('button', { name: 'Model and profile', exact: true });
      await expect(trigger).toBeEnabled();
      if (acknowledgement === 'lost') wire.loseNext('session/setModel');
      await trigger.click();
      await page.getByRole('menuitem', { name: 'Profile', exact: true }).click();
      await page.getByRole('menuitem', { name: 'low', exact: true }).click();
      if (acknowledgement === 'lost') {
        await expect.poll(wire.lost).toBe(1);
        await connectRemote(page, fixture.endpoint, fixture.token);
      }
      await expect(trigger).toContainText('low');
      await expect(running).toHaveText('Running: fixture/integration-model / high');
      await running.click();
      const details = page.locator('[data-running-model-details]');
      await expect(details.locator('[data-session-model]')).toHaveText('Session selection: fixture/integration-model / low');
      await expect(details.getByText(/Frozen at admission/)).toBeVisible();
      const bounds = await details.boundingBox();
      expect(bounds).not.toBeNull();
      expect(bounds!.x).toBeGreaterThanOrEqual(0);
      expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width);
      await expect(trigger).toBeVisible();
      const frozen = JSON.parse(await details.locator('pre').innerText());
      expect(frozen.primary.profile).toBe('high');
      expect(frozen.primary.reasoningEnabled).toBe(true);
      expect(frozen.primary.requestParams.reasoning_effort).toBe('high');
      expect(frozen.primary.maxOutputTokens).toBe(4096);
      expect(frozen.summary.mode).toBe('session');
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: test.info().outputPath('live-model.png') });
      await running.click();
      expect(wire.requests.filter(request => request.method === 'session/setModel')).toHaveLength(1);

      // Reload while the original request remains physically active. Native
      // attach must restore both authorities, with no mutation replay.
      await page.reload();
      await connectRemote(page, fixture.endpoint, fixture.token);
      const expand = page.getByRole('button', { name: 'Expand Sidebar', exact: true });
      if (await expand.isVisible()) await expand.click();
      await page.locator(`button[data-session-id="${sessionId}"]`).click();
      // Let the native attachment publish its route before dismissing the
      // sidebar: dismissing a pending navigation deliberately cancels it.
      await expect(page.locator('#session-view')).toHaveAttribute('data-shell-route', 'session');
      if (width < 600 && await collapse.isVisible()) await collapse.click();
      await expect(trigger).toContainText('low');
      await expect(running).toHaveText('Running: fixture/integration-model / high');
      expect(wire.requests.filter(request => request.method === 'session/setModel')).toHaveLength(1);
      await fixture.release('session-a-holding');
      await expect(running).toHaveCount(0);
      try { await expect(input).toBeEnabled(); }
      catch (cause) {
        await showInspector(page);
        await test.info().attach('recovery-authority', { body: await page.getByLabel('Native diagnostic JSON').innerText(), contentType: 'application/json' });
        throw cause;
      }
      await input.fill('tui multi-session: session B quick task');
      await input.press('Enter');
      await expect(page.getByText('B answered while A was still working', { exact: true })).toBeVisible();
      const captured = await fixture.control('requests') as { requests: { body: Record<string, unknown> }[] };
      expect(captured.requests).toHaveLength(2);
      const [high, low] = captured.requests.map(request => request.body);
      expect(high.reasoning_effort).toBe('high');
      expect(low.reasoning_effort).toBeUndefined();
      expect(high.max_tokens ?? high.max_completion_tokens).toBe(4096);
      expect(low.max_tokens ?? low.max_completion_tokens).toBe(1024);
      expect(errors).toEqual([]);
      passed = true;
    } finally { await fixture.stop(passed); }
  });
}
