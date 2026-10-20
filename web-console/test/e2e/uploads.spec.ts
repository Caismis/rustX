import { selectedSessionId, selectedCheckpoint } from './shell-actions';
import { openEmptySession } from './shell-actions';
import { connectRemote } from './shell-actions';
import { closeSessionView } from './shell-actions';
import { expectSettled } from './shell-actions';
import { expect, test } from '@playwright/test';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { wireProbe } from './wire-probe';
import { AppServerHost } from '../../../tui/src/app-server/host';

test('Session uploads compose with model Tool IO, fork, source deletion and reload', async ({ page }) => {
  const fixture = await startDogfood('web_upload_conformance');
  const wire = await wireProbe(page);
  let passed = false;
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  const message = page.getByRole('textbox', { name: 'Message', exact: true });
  const canonical = page.getByLabel('Canonical conversation');
  const id = () => selectedSessionId(page);
  const connect = async () => {
    await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(page.getByLabel('Transport token')).toHaveCount(0);
  };
  const submit = async (phase: string) => {
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await page.getByRole('button', { name: 'Allow once' }).click();
    await expect(canonical.getByText(`${phase} upload read through native Tool.`, { exact: true })).toBeVisible();
    await expectSettled(page, wire);
  };
  const root = (session: string) => join(fixture.workspaceA, '.agents/uploads', session);
  const paths = (session: string) => {
    const batches = readdirSync(root(session)); expect(batches).toHaveLength(2);
    return ['acceptance.txt', 'pixel.png'].map(name => { const matches = batches.map(batch => join(root(session), batch, name)).filter(existsSync); expect(matches).toHaveLength(1); return matches[0]; });
  };
  try {
    await routeWorkspaceHost(page, fixture); await page.goto('/'); await connect();
    await openEmptySession(page, fixture, 'Workspace A');
    await selectedCheckpoint(page, wire);
    await expect(message).toBeEnabled(); const source = await id();
    await message.fill('Use my uploaded files');
    const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6pAAAAABJRU5ErkJggg==', 'base64');
    await page.getByLabel('Attach files').setInputFiles([
      { name: 'acceptance.txt', mimeType: 'text/plain', buffer: Buffer.from('UPLOAD_NATIVE_SENTINEL') },
      { name: 'pixel.png', mimeType: 'image/png', buffer: png },
    ]);
    await expect(page.locator('[data-attachment-state=ready]')).toHaveCount(2);
    const sourcePaths = paths(source);
    expect(readFileSync(sourcePaths[0], 'utf8')).toBe('UPLOAD_NATIVE_SENTINEL');
    expect(readFileSync(sourcePaths[1])).toEqual(png);
    await submit('Source');
    // A native receipt, not a browser-constructed ArtifactId, enters turn/start.
    const input = wire.requests.find(request => request.method === 'turn/start')!.params.content;
    expect(input.filter((block: any) => block.type === 'upload')).toHaveLength(2);
    expect(JSON.stringify(input)).not.toContain('artifact');
    const sourceRequest = (await fixture.control('requests')).requests[0].body;
    const projected = sourceRequest.messages.filter((entry: any) => entry.role === 'user').flatMap((entry: any) => typeof entry.content === 'string' ? [entry.content] : entry.content.filter((block: any) => block.type === 'text').map((block: any) => block.text)).find((text: string) => text.includes('<user_uploaded_files>'));
    expect(projected).toContain('<user_uploaded_files>');
    expect(projected).toMatch(/<\/user_uploaded_files>\s*Use my uploaded files$/);
    for (const path of sourcePaths) expect(projected).toContain(path);

    await page.reload(); await connect();
    await expect(canonical.getByText('acceptance.txt', { exact: true })).toBeVisible();
    await expect(canonical.getByText('pixel.png', { exact: true })).toBeVisible();
    await message.fill('/fork'); await message.press('Enter');
    await page.getByRole('dialog', { name: '/fork', exact: true }).getByRole('option', { name: /Use my uploaded files/ }).click();
    await expect(message).toHaveValue('Use my uploaded files');
    const destination = await id(); expect(destination).not.toBe(source);
    const destinationPaths = paths(destination);
    expect(readFileSync(destinationPaths[0], 'utf8')).toBe('UPLOAD_NATIVE_SENTINEL');
    expect(readFileSync(destinationPaths[1])).toEqual(png);
    // Release the source controller without abandoning the destination editor.
    // Another client can inspect the source; deletion owns its retirement.
    const detached = wire.responses.filter(row => row.method === 'session/detach').length;
    await closeSessionView(page, source);
    await expect.poll(() => wire.responses.filter(row => row.method === 'session/detach').length).toBe(detached + 1);
    const observer = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    try {
      const attached = await observer.client.call('session/attach', { session_id: source }, 'attached');
      await observer.client.call('session/detach', { target: attached.target }, 'detached');
    } finally { await observer.shutdown(); }
    await page.locator(`button[data-session-id="${source}"]`).hover();
    await page.locator(`button[data-session-actions="${source}"]`).click();
    await page.getByRole('menuitem', { name: 'Delete Session', exact: true }).click();
    await page.getByRole('button', { name: 'Delete', exact: true }).click();
    await expect(page.getByRole('dialog', { name: 'Delete this session?' })).toHaveCount(0);
    await expect(page.getByText('Session deleted.', { exact: true })).toHaveCount(0);
    expect(existsSync(root(source))).toBe(false);
    for (const path of destinationPaths) expect(existsSync(path)).toBe(true);
    await expect(message).toHaveValue('Use my uploaded files');
    await submit('Destination');
    const requests = (await fixture.control('requests')).requests;
    expect(requests).toHaveLength(4);
    const destinationRequest = JSON.stringify(requests[2].body);
    for (const path of destinationPaths) expect(destinationRequest).toContain(path);
    for (const path of sourcePaths) expect(destinationRequest).not.toContain(path);
    // The real native Tool result contains the exact execution-world file path.
    expect(JSON.stringify(requests[3].body.messages.filter((entry: any) => entry.role === 'tool'))).toContain(destinationPaths[0]);
    await page.reload(); await connect();
    await expect(canonical.getByText('Destination upload read through native Tool.', { exact: true })).toBeVisible();
    await expect(canonical.getByText('acceptance.txt', { exact: true })).toBeVisible();
    expect(wire.requests.filter(request => request.method === 'artifact/read')).toHaveLength(0);
    await page.locator(`button[data-session-id="${destination}"]`).hover();
    await page.locator(`button[data-session-actions="${destination}"]`).click();
    await page.getByRole('menuitem', { name: 'Delete Session', exact: true }).click();
    await page.getByRole('button', { name: 'Delete', exact: true }).click();
    await expect(page.getByRole('dialog', { name: 'Delete this session?' })).toHaveCount(0);
    await expect(page.getByText('Session deleted.', { exact: true })).toHaveCount(0);
    expect(existsSync(root(destination))).toBe(false);
    expect(errors).toEqual([]); passed = true;
  } finally { await page.close(); await fixture.stop(passed); }
});

test('24 files including a six MiB document use the binary lane without a model request', async ({ page }) => {
  const fixture = await startDogfood('web_upload_conformance');
  const wire = await wireProbe(page);
  try {
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A');
    await selectedCheckpoint(page, wire);
    const session = await selectedSessionId(page);
    const document = Buffer.alloc(6 * 1024 * 1024, 'a'); document.write('# Ordinary Markdown document\n');
    await page.getByLabel('Attach files').setInputFiles([{ name: 'large-document.md', mimeType: 'text/markdown', buffer: document }, ...Array.from({ length: 23 }, (_, index) => ({ name: `note-${index}.txt`, mimeType: 'text/plain', buffer: Buffer.from(`note ${index}`) }))]);
    await expect(page.locator('[data-attachment-state=ready]')).toHaveCount(24);
    const root = join(fixture.workspaceA, '.agents/uploads', session); const batches = readdirSync(root);
    expect(batches).toHaveLength(24);
    const largeBatch = batches.find(batch => existsSync(join(root, batch, 'large-document.md')))!;
    expect(readFileSync(join(root, largeBatch, 'large-document.md'))).toEqual(document);
    for (let index = 0; index < 23; index++) {
      const name = `note-${index}.txt`, batch = batches.find(batch => existsSync(join(root, batch, name)))!;
      expect(readFileSync(join(root, batch, name), 'utf8')).toBe(`note ${index}`);
    }
    expect(wire.requests.filter(request => request.method === 'session/uploadPrepare')).toHaveLength(24);
    expect(wire.requests.filter(request => request.method === 'turn/start')).toHaveLength(0);
    expect((await fixture.control('requests')).requests).toHaveLength(0);
  } finally { await page.close(); const report = await fixture.stop(false); expect(report.requestCount).toBe(0); expect(report.failures).toEqual([]); }
});

test('32 independent uploads keep their 33rd text block in live and reopened native Chat', async ({ page }) => {
  const fixture = await startDogfood('web_upload_conformance');
  const wire = await wireProbe(page), errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  try {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A'); await selectedCheckpoint(page, wire);
    const input = page.getByRole('textbox', { name: 'Message', exact: true });
    const chat = page.getByLabel('Canonical conversation');
    await input.fill('Use my uploaded files');
    await page.getByLabel('Attach files').setInputFiles([
      { name: 'acceptance.txt', mimeType: 'text/plain', buffer: Buffer.from('UPLOAD_NATIVE_SENTINEL') },
      ...Array.from({ length: 31 }, (_, index) => ({ name: `continuation-${index}.txt`, mimeType: 'text/plain', buffer: Buffer.from(`file ${index}`) })),
    ]);
    await expect(page.locator('[data-attachment-state=ready]')).toHaveCount(32);
    const prepares = wire.requests.filter(request => request.method === 'session/uploadPrepare');
    expect(prepares).toHaveLength(32); expect(prepares.every(request => request.params.files.length === 1)).toBe(true);
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await page.getByRole('button', { name: 'Allow once', exact: true }).click();
    await expect(chat.getByText('Source upload read through native Tool.', { exact: true })).toBeVisible();
    await expectSettled(page, wire);
    await expect(chat.getByText('Use my uploaded files', { exact: true })).toBeAttached();
    await expect(chat.locator('[data-message-file]')).toHaveCount(32);
    await page.reload(); await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(chat.getByText('Use my uploaded files', { exact: true })).toBeAttached();
    await expect(chat.locator('[data-message-file]')).toHaveCount(32);
    await expect(page).toHaveTitle(/rustX/); await expect(chat).toBeVisible();
    await page.screenshot({ path: '/tmp/rustx-chat-33-blocks-1440.png' });
    const collapse = page.getByRole('button', { name: 'Collapse Sidebar', exact: true });
    if (await collapse.isVisible()) await collapse.click();
    await page.setViewportSize({ width: 390, height: 844 });
    if (await collapse.isVisible()) await collapse.click();
    // The collapsed rail and frame borders leave over 320px for the viewport.
    // Measure the viewport rather than its intentionally padded message column.
    const viewport = page.locator('[data-conversation-scroll]').first();
    await expect.poll(async () => (await viewport.boundingBox())?.width ?? 0).toBeGreaterThan(320);
    await expect(chat.getByText('Use my uploaded files', { exact: true })).toBeVisible();
    expect((await chat.getByText('Use my uploaded files', { exact: true }).boundingBox())!.height).toBeLessThan(40);
    await expect(chat).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: '/tmp/rustx-chat-33-blocks-390.png' });
    await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
  } finally { await page.close(); const report = await fixture.stop(false); expect(report.failures).toEqual([]); }
});
