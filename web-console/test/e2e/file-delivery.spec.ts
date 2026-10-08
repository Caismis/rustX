import { test, expect, type Locator } from '@playwright/test';
import { readFileSync, writeFileSync, unlinkSync } from 'node:fs';
import { join } from 'node:path';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openEmptySession, expectSettled, choose } from './shell-actions';
import { wireProbe } from './wire-probe';

test('explicit native delivery, safe shared viewers, actual original-byte downloads, mutable reopen and both layouts/locales', async ({ page }, testInfo) => {
  const fixture = await startDogfood('web_file_delivery');
  let passed = false;
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  const wire = await wireProbe(page);
  let deliveryRead: any;
  page.on('request', request => { if (request.url().endsWith('/product-host/file-read')) deliveryRead = request.postDataJSON().read; });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.addInitScript(() => {
    const urls = new Set<string>(), create = URL.createObjectURL, revoke = URL.revokeObjectURL;
    URL.createObjectURL = blob => { const url = create(blob); urls.add(url); return url; };
    URL.revokeObjectURL = url => { urls.delete(url); revoke(url); };
    Object.assign(window, { deliveryUrls: urls });
  });
  const countUrls = () => page.evaluate(() => (window as any).deliveryUrls.size);
  const canonical = page.getByLabel(/^(Canonical conversation|规范对话)$/);
  const panel = page.getByRole('complementary', { name: 'Previews', exact: true });
  const card = (name: string) => canonical.locator('[data-delivery-card]').filter({ has: page.locator('[data-presented-name]', { hasText: name }) }).first();
  const close = async () => { await panel.getByRole('button', { name: /^Close preview / }).first().click(); await expect.poll(countUrls).toBe(0); };
  const download = async (link: Locator, name: string, expected: Buffer) => {
    const event = page.waitForEvent('download'); await link.click(); const file = await event;
    expect(file.suggestedFilename()).toBe(name); const path = testInfo.outputPath(`download-${encodeURIComponent(name)}`); await file.saveAs(path); expect(readFileSync(path)).toEqual(expected);
  };
  try {
    await routeWorkspaceHost(page, fixture); await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Deliver files explicitly');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Approval', exact: true })).toContainText('write:');
    await page.getByRole('button', { name: 'Allow once', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Approval', exact: true })).toContainText('bash:');
    await page.getByRole('button', { name: 'Allow once', exact: true }).click();
    await fixture.gate('delivery-before-declaration');
    await expect(canonical.getByText('I created report.md and 报告 file.md.', { exact: true })).toBeVisible();
    await expect(canonical.locator('[data-delivery-card]')).toHaveCount(0);
    await expect(canonical.locator('a[download]')).toHaveCount(0);
    await fixture.release('delivery-before-declaration');
    await expect(canonical.getByText('Delivery finished.', { exact: true })).toBeVisible(); await expectSettled(page);
    // Harness summary: four cards per committed result until explicitly expanded.
    await expect(canonical.locator('[data-delivery-card]')).toHaveCount(5);
    const all = canonical.getByRole('button', { name: 'Show all 6 delivered files', exact: true });
    await expect(all).toHaveAttribute('aria-expanded', 'false');
    await all.click();
    await expect(canonical.locator('[data-delivery-card]')).toHaveCount(7);
    expect(await canonical.locator('[data-delivery-card] [data-presented-name]').allTextContents()).toEqual(['报告 file.md', 'plain 空格.txt', 'source.rs', 'pixel.png', 'data.bin', 'large.txt', '报告 file.md']);
    // The present call rows report runtime-owned phases; cards come only from committed results.
    await expect(canonical.locator('[data-tool="present"][data-present-phase="error"]')).toHaveCount(1);
    await expect(canonical.locator('[data-tool="present"][data-present-phase="ok"]')).toHaveCount(2);
    expect(await canonical.innerText()).not.toContain('Duplicate discarded');
    // Cards remain visible while completed activity is folded.
    await expect(card('报告 file.md')).toBeVisible();
    const modelRequests = (await fixture.control('requests')).requests.length; expect(modelRequests).toBe(6);
    const original = readFileSync(join(fixture.workspaceA, '报告 file.md'));
    await card('报告 file.md').getByRole('button', { name: 'Preview 报告 file.md in sidebar', exact: true }).focus();
    await page.keyboard.press('Enter');
    await expect(panel.getByRole('heading', { name: 'Delivered report' })).toBeVisible();
    await expect(panel.locator('strong', { hasText: 'Original' })).toBeVisible();
    expect(await page.evaluate(() => (window as any).PWNED)).toBeUndefined();
    await expect(panel.locator('script,img,iframe,object')).toHaveCount(0);
    await download(panel.getByRole('button', { name: 'Download artifact', exact: true }), '报告 file.md', original);
    expect(deliveryRead).toBeDefined();
    const bypass = await page.evaluate(({ endpoint, token, read, cwd }) => new Promise<number>((resolve, reject) => {
      const socket = new WebSocket(endpoint, ['rustx.app-server.v38', `rustx-token.${token}`]);
      socket.onerror = () => reject(new Error('Browser ordinary connection failed'));
      socket.onopen = () => socket.send(JSON.stringify({ jsonrpc: '2.0', id: 4191, method: 'initialize', params: { protocol_version: 38, client: { name: 'rustx-product-host-file-read', version: '1' }, presentation: { images: true, questionnaires: true, reviews: true } } }));
      socket.onmessage = event => {
        const reply = JSON.parse(event.data);
        if (reply.id === 4191) socket.send(JSON.stringify({ jsonrpc: '2.0', id: 4192, method: 'session/fileRead', params: { ...read, allowed_roots: [cwd] } }));
        else if (reply.id === 4192) socket.send(JSON.stringify({ jsonrpc: '2.0', id: 4193, method: 'delivery/read', params: read }));
        else if (reply.id === 4193) { socket.close(); resolve(reply.error?.data?.kind === 'session_file_read' && reply.error.data.reason === 'unauthorized' ? -32601 : 0); }
      };
    }), { endpoint: fixture.endpoint, token: fixture.token, read: deliveryRead, cwd: fixture.workspaceA });
    // All legitimate coordinates and exact cwd still confer no Host authority; the
    // removed method stays absent and delivery/read without transport-granted access fails closed.
    expect(bypass).toBe(-32601);
    await close();
    for (const name of ['plain 空格.txt', 'source.rs', 'pixel.png', 'data.bin', 'large.txt']) {
      await card(name).getByRole('button', { name: `Preview ${name} in sidebar`, exact: true }).click();
      if (name === 'pixel.png') { await expect(panel.getByRole('img')).toBeVisible(); expect(await panel.getByRole('img').evaluate((img: HTMLImageElement) => img.naturalWidth)).toBe(1); }
      else if (name === 'data.bin') await expect(panel.getByText('This artifact has no supported inline viewer.')).toBeVisible();
      else await expect(panel.locator('pre')).toBeVisible();
      await download(panel.getByRole('button', { name: 'Download artifact', exact: true }), name, readFileSync(join(fixture.workspaceA, name))); await close();
    }
    // A card's direct Download uses the same owner and the native current bytes.
    await download(card('plain 空格.txt').getByRole('button', { name: 'Download plain 空格.txt', exact: true }), 'plain 空格.txt', readFileSync(join(fixture.workspaceA, 'plain 空格.txt')));
    await expect(panel).not.toBeVisible();
    await expect.poll(countUrls).toBe(0);
    writeFileSync(join(fixture.workspaceA, '报告 file.md'), '# Current mutable report\n');
    await card('报告 file.md').getByRole('button', { name: 'Preview 报告 file.md in sidebar', exact: true }).click();
    await expect(panel.getByRole('heading', { name: 'Current mutable report' })).toBeVisible(); await close();
    unlinkSync(join(fixture.workspaceA, '报告 file.md'));
    await card('报告 file.md').getByRole('button', { name: 'Preview 报告 file.md in sidebar', exact: true }).click();
    await expect(panel.getByRole('alert')).toContainText('file is missing'); await expect(panel.getByRole('button', { name: 'Download artifact', exact: true })).toHaveCount(0);
    writeFileSync(join(fixture.workspaceA, '报告 file.md'), original);
    await panel.getByRole('button', { name: 'Retry preview' }).click();
    await expect(panel.getByRole('heading', { name: 'Delivered report' })).toBeVisible(); await close();
    // The existing measured conversation and turn navigator remain the scroll owners.
    const viewport = page.locator('.conversation-scroll');
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 1000 });
      await card('source.rs').scrollIntoViewIfNeeded();
      const before = await viewport.evaluate(el => el.scrollTop);
      await card('source.rs').getByRole('button', { name: 'Preview source.rs in sidebar', exact: true }).click();
      await expect(panel.locator('pre')).toBeVisible(); await close();
      await expect.poll(() => viewport.evaluate(el => el.scrollTop)).toBeCloseTo(before, 0);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    }
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文');
    await page.getByRole('button', { name: '关闭设置', exact: true }).click();
    await card('source.rs').getByRole('button', { name: '在侧边栏预览 source.rs', exact: true }).click();
    const chinese = page.getByRole('complementary', { name: '预览', exact: true });
    await expect(chinese.locator('pre')).toBeVisible();
    await download(chinese.getByRole('button', { name: '下载制品', exact: true }), 'source.rs', readFileSync(join(fixture.workspaceA, 'source.rs')));
    await chinese.getByRole('button', { name: /^关闭预览 / }).first().click(); await expect.poll(countUrls).toBe(0);
    const hostScope = await fixture.workspaceHost.host.listWorkspaces();
    await fixture.workspaceHost.host.removeWorkspace(hostScope, hostScope.workspaces.find(row => row.location === 'root-a')!.id);
    await card('source.rs').getByRole('button', { name: '在侧边栏预览 source.rs', exact: true }).click();
    await expect(chinese.getByRole('alert')).toContainText('not authorized');
    await expect(chinese.getByRole('button', { name: '下载制品', exact: true })).toHaveCount(0);
    await chinese.getByRole('button', { name: /^关闭预览 / }).first().click(); await expect.poll(countUrls).toBe(0);
    expect((await fixture.control('requests')).requests).toHaveLength(modelRequests);
    expect(wire.requests.filter(request => request.method === 'turn/start')).toHaveLength(1);
    expect(errors).toEqual([]); passed = true;
  } finally { await fixture.stop(passed); }
});

test('managed Artifact Markdown/text/code previews download original bytes through artifact/read', async ({ page }, testInfo) => {
  await page.goto(`http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/file-preview.html`);
  const bytes = Buffer.from('# Managed report\r\n\r\nOriginal **bytes**: 报告\r\n');
  for (const extension of ['md', 'txt', 'rs']) {
    const name = `报告 managed file.${extension}`;
    await page.getByRole('button', { name: `Preview ${name}`, exact: true }).click();
    if (extension === 'md') await expect(page.getByRole('heading', { name: 'Managed report' })).toBeVisible();
    else await expect(page.locator('pre')).toContainText('Original **bytes**');
    const event = page.waitForEvent('download'); await page.getByRole('button', { name: 'Download artifact', exact: true }).click(); const download = await event;
    expect(download.suggestedFilename()).toBe(name); const path = testInfo.outputPath(`download-${extension}`); await download.saveAs(path); expect(readFileSync(path)).toEqual(bytes);
    await page.getByRole('button', { name: 'Close preview', exact: true }).click();
  }
  const methods = await page.evaluate(() => (window as any).managedPreviewRequests.map((value: any) => value.request.method));
  expect(methods.filter((method: string) => method === 'artifact/read')).toHaveLength(6); // Preview and Download each authorize a fresh read.
  expect(methods.filter((method: string) => method === 'session/fileRead' || method === 'turn/start')).toHaveLength(0);
});
