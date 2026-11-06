import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openEmptySession } from './shell-actions';

test('large native write remains expandable after settlement and reload', async ({ page }) => {
  const fixture = await startDogfood('web_large_write');
  let passed = false;
  const evidence: unknown[] = [], errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  await page.routeWebSocket(/ws:\/\//, socket => {
    const server = socket.connectToServer();
    socket.onMessage(message => server.send(message));
    server.onMessage(message => {
      const value = JSON.parse(String(message));
      if (value.params?.event?.type === 'projection_transition') {
        for (const event of value.params.event.events) if (event.type === 'message_descriptor_committed') evidence.push(event.descriptor);
      }
      socket.send(message);
    });
  });
  try {
    await page.setViewportSize({ width: 1440, height: 1000 });
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A');
    const message = page.getByRole('textbox', { name: 'Message', exact: true });
    await message.fill('Write the native report'); await message.press('Enter');
    await expect(page.getByText('Large write finished.', { exact: true })).toBeVisible();
    const process = page.getByRole('button', { name: /^Took/ });
    await process.click();
    const step = page.getByRole('button', { name: /Wrote.*called|Wrote.*tool|Wrote/ });
    if (await step.count()) await step.first().click();
    await expect(page.getByText('Assembling write…', { exact: true })).toHaveCount(0);
    const call = page.locator('[data-tool-call-id="large-write"]');
    await expect(call).toBeVisible();
    await call.getByRole('button', { name: /^write/ }).click();
    await expect(call.getByText('Native write report', { exact: false })).toBeVisible();
    await page.screenshot({ path: '/tmp/rustx-large-write.png' });
    await page.reload();
    await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(page.getByText('Large write finished.', { exact: true })).toBeVisible();
    await process.click();
    if (await step.count()) await step.first().click();
    await expect(page.getByText('Assembling write…', { exact: true })).toHaveCount(0);
    await call.getByRole('button', { name: /^write/ }).click();
    await expect(call.getByText('Native write report', { exact: false })).toBeVisible();
    await page.setViewportSize({ width: 390, height: 1000 });
    await call.scrollIntoViewIfNeeded();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: '/tmp/rustx-large-write-reloaded-390.png' });
    expect(errors).toEqual([]);
    passed = true;
  } catch (error) {
    await test.info().attach('native-write-descriptors', { body: JSON.stringify(evidence), contentType: 'application/json' });
    await page.screenshot({ path: '/tmp/rustx-large-write-failure.png' });
    throw error;
  } finally { await fixture.stop(passed); }
});
