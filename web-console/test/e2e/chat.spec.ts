import { connectRemote } from './shell-actions';
import { connectionAction, closeSettings, showInspector, expectSettled } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';
import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { AppServerHost } from '../../../tui/src/app-server/host';

test('native history, rich settlement, real image decode/lightbox, reconnect and native upload receipts', async ({ page }) => {
  const fixture = await startDogfood('web_chat_history');
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.addInitScript(() => {
    const urls = new Set<string>();
    const create = URL.createObjectURL.bind(URL), revoke = URL.revokeObjectURL.bind(URL);
    URL.createObjectURL = value => { const url = create(value); urls.add(url); return url; };
    URL.revokeObjectURL = url => { urls.delete(url); revoke(url); };
    Object.defineProperty(window, 'acceptanceObjectUrls', { get: () => urls.size });
  });
  const urlCount = () => page.evaluate(() => (window as unknown as { acceptanceObjectUrls: number }).acceptanceObjectUrls);
  let passed = false;
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    // Build durable history via typed native admission before the browser attaches.
    const remote = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    const created = await remote.client.call('session/create', { settings: { cwd: fixture.workspaceA } }, 'session_transition');
    const id = created.session.id;
    const attached = await remote.client.call('session/attach', { session_id: id }, 'attached');
    await expect(remote.client.call('artifact/read', { target: attached.target, artifact_id: 'artifact_999' }, 'artifact_bytes')).rejects.toThrow();
    for (let i = 0; i < 34; i++) {
      await remote.client.call('turn/start', { target: attached.target, content: [{ type: 'text', text: `History ${i}` }] }, 'inbound_accepted');
      await expect.poll(async () => {
        const current = await remote.client.call('session/snapshot', { target: attached.target }, 'snapshot');
        return current.snapshot.attempt?.phase.type === 'settled' && current.snapshot.messages.some(message => message.role === 'assistant' && message.content.some(block => block.type === 'text' && block.text === `Answer ${i}`));
      }).toBe(true);
    }
    const traceBeforeBrowser = (await remote.client.call('session/trace', { target: attached.target, limit: 32 }, 'trace')).page;
    expect(traceBeforeBrowser.records).toHaveLength(32);
    await remote.client.call('session/detach', { target: attached.target }, 'detached');
    await remote.shutdown();
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await page.locator(`button[data-session-id="${id}"]`).click();
    await expect(page.getByRole('button', { name: 'Load earlier', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Load earlier', exact: true }).scrollIntoViewIfNeeded();
    const firstKey = await page.locator('[data-chat-anchor-key]').first().getAttribute('data-chat-anchor-key');
    const anchor = page.locator(`[data-chat-anchor-key=${JSON.stringify(firstKey)}]`);
    const anchorTop = await anchor.evaluate(el => el.getBoundingClientRect().top);
    await expect(page.getByText('Answer 0', { exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Load earlier', exact: true }).click();
    await expect(page.getByText('Answer 0', { exact: true })).toHaveCount(1);
    await expect(page.getByRole('button', { name: 'Load earlier', exact: true })).toHaveCount(0);
    expect(Math.abs(await anchor.evaluate(el => el.getBoundingClientRect().top) - anchorTop)).toBeLessThan(2);
    // Historical tails retain native identity after paging; pointer reveal also
    // exposes equivalent keyboard controls without a permanent wide toolbar.
    await expect(page.getByLabel('Completed Turn', { exact: true })).toHaveCount(34);
    const oldTail = page.getByLabel('Completed Turn', { exact: true }).first();
    const oldActions = oldTail;
    await page.getByLabel('Message', { exact: true }).focus();
    await page.mouse.move(0, 0);
    await expect(oldActions).toHaveCSS('opacity', '0');
    await oldTail.getByRole('button', { name: 'Copy', exact: true }).focus();
    await expect(oldActions).toHaveCSS('opacity', '1');
    await oldTail.hover();
    await expect(oldActions).toHaveCSS('opacity', '1');
    await page.setViewportSize({ width: 390, height: 844 });
    await oldTail.getByRole('button', { name: 'Branch into a new Session', exact: true }).focus();
    await expect(oldTail.getByRole('button', { name: 'Branch into a new Session', exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: 'test-results/response-tail-mobile.png' });
    // The Harness ledger seats a Request on its output row, so two finite
    // pages need a shorter pane to overflow by a real scrolling extent.
    await page.setViewportSize({ width: 1440, height: 800 });
    // WEB-03 uses this same native Session, transcript and provider scenario.
    await page.getByRole('tab', { name: 'Trajectory', exact: true }).click();
    const trajectory = page.getByRole('region', { name: 'Trajectory', exact: true });
    const ledger = trajectory.getByRole('table', { name: 'Trace ledger' });
    await expect(trajectory.locator('[data-request-owner]').first()).toBeVisible();
    await expect(ledger.locator('[data-display-type="TurnHeader"], [data-display-type="GroupHeader"]')).toHaveCount(0);
    await expect.poll(() => ledger.evaluate(el => el.scrollHeight - el.clientHeight - el.scrollTop)).toBeLessThan(2);
    // Compact summaries can fit the whole first page without scrolling. Load
    // one more page to establish a real scrolling viewport before asserting
    // prepend anchoring (a short viewport necessarily clamps scrollTop to 0).
    await trajectory.getByRole('region', { name: 'Trajectory timeline' }).getByRole('button', { name: 'Load earlier history', exact: true }).click();
    await expect.poll(() => ledger.evaluate(el => el.scrollHeight - el.clientHeight)).toBeGreaterThan(200);
    await ledger.evaluate(el => { el.scrollTop = 100; el.dispatchEvent(new Event('scroll')); });
    const traceAnchorId = await ledger.evaluate(el => Array.from(el.querySelectorAll<HTMLElement>('[data-request-owner]')).find(marker => marker.getBoundingClientRect().top >= el.getBoundingClientRect().top)!.dataset.requestOwner!);
    const traceAnchor = trajectory.locator(`[data-request-owner="${traceAnchorId}"]`);
    const traceAnchorTop = await traceAnchor.evaluate(el => el.getBoundingClientRect().top);
    for (let pageNumber = 0; pageNumber < 4; pageNumber++) {
      const before = Number(await ledger.getAttribute('aria-rowcount'));
      await trajectory.getByRole('region', { name: 'Trajectory timeline' }).getByRole('button', { name: 'Load earlier history', exact: true }).click();
      await expect.poll(async () => Number(await ledger.getAttribute('aria-rowcount'))).toBeGreaterThan(before);
    }
    const capacity = await ledger.evaluate(el => Math.ceil(el.clientHeight / 10) + 2 * 12 + 1);
    expect(await ledger.getByRole('row').count()).toBeLessThanOrEqual(capacity);
    await expect.poll(async () => Math.abs(await trajectory.locator(`[data-request-owner="${traceAnchorId}"]`).evaluate(el => el.getBoundingClientRect().top) - traceAnchorTop)).toBeLessThan(2);
    const requestRecord = traceBeforeBrowser.records.find(record => record.request)!;
    await trajectory.getByLabel('Search trajectory').fill(requestRecord.request!.model);
    await trajectory.locator(`[data-request-owner="${requestRecord.id}"]`).click();
    const inspector = trajectory.getByRole('complementary', { name: 'Event details' });
    await expect(inspector.getByRole('tab')).toHaveText(['Summary', 'Options', 'Usage', 'Timing']);
    await inspector.getByRole('tab', { name: 'Options', exact: true }).click();
    await expect(inspector).toContainText(requestRecord.request!.model);
    await page.screenshot({ path: '/tmp/rustx-364-trajectory-desktop.png' });
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(inspector).toBeVisible();
    await page.screenshot({ path: '/tmp/rustx-364-trajectory-mobile.png' });
    await page.setViewportSize({ width: 1440, height: 1000 });
    await inspector.getByRole('tab', { name: 'Options', exact: true }).click();
    await expect(inspector).toContainText(requestRecord.request!.model);
    // Infrastructure authority still does not cross the boundary.
    expect(await inspector.innerText()).not.toContain(fixture.workspaceA);
    await trajectory.getByLabel('Search trajectory').fill('');
    await inspector.getByRole('button', { name: 'Close details' }).click();
    await closeSettings(page); await page.getByRole('tab', { name: 'Chat', exact: true }).click();
    await page.getByLabel('Message', { exact: true }).fill('Rich reply');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await fixture.gate('settle-chat');
    await expect(page.getByRole('heading', { name: 'Rich reply' })).toHaveCount(1);
    // Reconstruct a bounded latest window while the same native attempt is
    // held mid-stream, then page its history without changing live ownership.
    await connectionAction(page, 'Reconnect');
    await expect(page.getByLabel('Transport token')).toHaveCount(0);
    await page.getByRole('button', { name: 'Load earlier', exact: true }).click();
    await expect(page.getByText('Answer 0', { exact: true })).toHaveCount(1);
    await expect(page.getByLabel('Session status')).toHaveCount(0);
    await expect(page.getByRole('button', { name: /^Deep diving/ }).first()).toBeVisible();

    await page.getByRole('tab', { name: 'Trajectory', exact: true }).click();
    await expect.poll(() => ledger.evaluate(el => el.scrollHeight - el.clientHeight - el.scrollTop)).toBeLessThan(2);
    await expect(trajectory.locator('[data-request-owner][data-state="running"]').first()).toBeVisible();
    // A reader away from the tail owns their position while live repair runs.
    await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
    await connectionAction(page, 'Reconnect');
    await expect(page.getByLabel('Transport token')).toHaveCount(0);
    await ledger.evaluate(el => { el.scrollTop = el.scrollHeight; el.dispatchEvent(new Event('scroll')); });
    await expect(trajectory.locator('[data-request-owner][data-state="running"]').first()).toBeVisible();
    await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
    // Settlement appends the running Request's Assistant output, which then
    // hosts its marker in place of the Request's own seat.
    const running = trajectory.locator('[data-request-owner][data-state="running"]');
    await expect(running).toHaveCount(1);
    const settling = trajectory.locator(`[data-request-owner="${await running.getAttribute('data-request-owner')}"]`);
    await fixture.release('settle-chat');
    await expect(settling).toHaveAttribute('data-state', 'completed');
    await expect(settling.locator('xpath=ancestor::*[@role="row"]')).toHaveAttribute('data-kind', 'assistant');
    expect(await ledger.evaluate(el => el.scrollTop)).toBe(0);
    await closeSettings(page); await page.getByRole('tab', { name: 'Chat', exact: true }).click();
    await expect(page.getByText('Settled', { exact: true })).toHaveCount(1);
    await expect(page.locator('[aria-label="Streaming response"]')).toHaveCount(0);
    await expect(page.getByRole('table')).toHaveCount(1);
    await connectionAction(page, 'Reconnect');
    await expect(page.getByLabel('Transport token')).toHaveCount(0);
    await expect(page.getByRole('heading', { name: 'Rich reply' })).toHaveCount(1);
    await page.getByRole('button', { name: 'Load earlier', exact: true }).click();
    await expect(page.getByText('Answer 0', { exact: true })).toHaveCount(1);
    await page.getByLabel('Message', { exact: true }).fill('Keep this draft');
    await page.getByLabel('Attach files').setInputFiles({ name: 'note.txt', mimeType: 'text/plain', buffer: Buffer.from('file content') });
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByText('Uploaded workspace file received.', { exact: true })).toBeVisible();
    await expect(page.getByLabel('Message', { exact: true })).toHaveValue('');
    await expect(page.getByLabel('Canonical conversation').getByText('note.txt', { exact: true })).toBeVisible();
    await connectionAction(page, 'Reconnect');
    await expect(page.getByLabel('Transport token')).toHaveCount(0);
    await expect(page.getByLabel('Canonical conversation').getByText('note.txt', { exact: true })).toBeVisible();
    await page.getByLabel('Message', { exact: true }).fill('Image please');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    const canonical = page.getByLabel('Canonical conversation');
    const load = canonical.getByRole('button', { name: 'Load attachment' });
    // Text-only continuation keeps a useful artifact reference while durable
    // image content remains available to the managed preview.
    await expectSettled(page);
    await expect(page.getByLabel('Native diagnostic JSON')).toContainText('completed');
    await page.getByRole('button', { name: 'Close Inspector' }).click();
    await expect(canonical.locator('[data-tool-call-id="chat-image"]')).toHaveCount(1);
    const expandImage = async () => {
      const process = canonical.locator('[data-turn-process]').last();
      if (await process.getAttribute('aria-expanded') === 'false') await process.click();
      // The tool row sits in its step group, collapsed once the Turn settles.
      const tool = canonical.locator('[data-tool-call-id="chat-image"]').last();
      const step = canonical.locator('[data-step-process]').filter({ has: page.locator('[data-tool-call-id="chat-image"]') }).last().locator(':scope > button');
      if (await step.getAttribute('aria-expanded') === 'false') await step.click();
      const disclosure = tool.getByRole('button', { expanded: false });
      if (await disclosure.count()) await disclosure.click();
    };
    await expandImage();
    await expect(load).toHaveCount(1);
    const decode = async () => {
      await load.click();
      const image = canonical.locator('img');
      await expect(image).toBeVisible();
      await expect.poll(() => image.evaluate((node: HTMLImageElement) => node.complete && node.naturalWidth > 0 && node.naturalHeight > 0)).toBe(true);
      await image.click();
      const original = page.getByRole('dialog').locator('img');
      await expect(original).toBeVisible();
      await expect.poll(() => original.evaluate((node: HTMLImageElement) => node.complete && node.naturalWidth > 0)).toBe(true);
      await page.keyboard.press('Escape');
    };
    await decode();
    expect(await urlCount()).toBe(1);
    await connectionAction(page, 'Reconnect');
    await expect(page.getByLabel('Transport token')).toHaveCount(0);
    await expandImage();
    await expect(load).toHaveCount(1);
    expect(await urlCount()).toBe(0);
    await decode();
    expect(await urlCount()).toBe(1);
    await canonical.getByRole('button', { name: /^Preview / }).click();
    const preview = page.getByRole('complementary', { name: 'Previews', exact: true });
    await expect(preview.getByRole('img')).toBeVisible();
    await expect.poll(() => preview.getByRole('img').evaluate((image: HTMLImageElement) => image.complete && image.naturalWidth > 0)).toBe(true);
    expect(await urlCount()).toBe(2);
    expect(await preview.evaluate(el => el.getBoundingClientRect().right <= innerWidth && el.getBoundingClientRect().left >= 0)).toBe(true);
    await page.screenshot({ path: 'test-results/native-artifact-preview.png' });
    await preview.getByRole('button', { name: /^Close preview / }).first().click();
    await expect.poll(urlCount).toBe(1);
    await page.getByRole('tab', { name: 'Trajectory', exact: true }).click();
    await expect.poll(urlCount).toBe(0);
    await trajectory.getByLabel('Search trajectory').fill('chat-image');
    const tool = trajectory.locator('[data-trace-id][data-kind="tool"]');
    await expect(tool).toHaveCount(1);
    const stableToolId = await tool.getAttribute('data-trace-id');
    await tool.click();
    // The selected native Tool owns the result; opaque call IDs are searchable
    // identity, not human-facing Inspector labels.
    await expect(inspector).toContainText('render_image');
    await expect(inspector.getByRole('button', { name: 'Preview artifact_1', exact: true })).toBeVisible();
    await page.screenshot({ path: 'test-results/trajectory-inspector.png', fullPage: true });
    await connectionAction(page, 'Reconnect');
    await expect(page.getByLabel('Transport token')).toHaveCount(0);
    await expect(trajectory.locator(`[data-trace-id="${stableToolId}"]`)).toHaveCount(1);
    expect(await trajectory.innerText()).not.toContain(fixture.workspaceA);
    await closeSettings(page); await page.getByRole('tab', { name: 'Chat', exact: true }).click();
    expect((await fixture.control('requests')).requests).toHaveLength(38);
    await page.screenshot({ path: 'test-results/chat-history.png', fullPage: true });
    // A Fork preserves independently readable managed images.
    const copiedImage = async () => {
      await expandImage();
      await expect(load).toHaveCount(1);
      await decode();
    };
    await page.getByRole('button', { name: 'Branch into a new Session', exact: true }).last().click();
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await copiedImage();
    expect(errors).toEqual([]); passed = true;
  } catch (error) {
    await showInspector(page);
    await test.info().attach('native-image-diagnostics', { body: JSON.stringify({ text: await page.locator('body').innerText(), diagnostics: fixture.diagnostics(), requests: await fixture.control('requests') }), contentType: 'application/json' });
    throw error;
  } finally { await page.close(); await fixture.stop(passed); }
});
