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
  const id = async () => JSON.parse(await page.getByLabel('Runtime facts').innerText()).SessionId as string;
  const connect = async () => {
    await page.getByLabel('WebSocket endpoint').fill(fixture.endpoint);
    await page.getByLabel('Transport token').fill(fixture.token);
    await page.getByRole('button', { name: 'Connect', exact: true }).click();
    await expect(page.locator('.status strong')).toHaveText('connected');
  };
  const submit = async (phase: string) => {
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await page.getByRole('button', { name: 'Allow once' }).click();
    await expect(canonical.getByText(`${phase} upload read through native Tool.`, { exact: true })).toBeVisible();
    await expect(page.locator('.attempt-status')).toContainText('settled');
  };
  const root = (session: string) => join(fixture.workspaceA, '.agents/uploads', session);
  const paths = (session: string) => {
    const batches = readdirSync(root(session)); expect(batches).toHaveLength(1);
    return ['acceptance.txt', 'pixel.png'].map(name => join(root(session), batches[0], name));
  };
  try {
    await routeWorkspaceHost(page, fixture); await page.goto('/'); await connect();
    await page.getByLabel('Choose Workspace').selectOption({ label: 'Workspace A' });
    await page.getByRole('button', { name: 'Create Session', exact: true }).click();
    await expect(message).toBeEnabled(); const source = await id();
    await message.fill('Use my uploaded files');
    const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6pAAAAABJRU5ErkJggg==', 'base64');
    await page.getByLabel('Attach files').setInputFiles([
      { name: 'acceptance.txt', mimeType: 'text/plain', buffer: Buffer.from('UPLOAD_NATIVE_SENTINEL') },
      { name: 'pixel.png', mimeType: 'image/png', buffer: png },
    ]);
    await expect(page.getByText('Uploaded', { exact: true })).toHaveCount(2);
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
    await page.getByRole('button', { name: 'Fork', exact: true }).click();
    await page.getByRole('dialog', { name: '/fork', exact: true }).getByRole('option', { name: /Use my uploaded files/ }).click();
    await expect(message).toHaveValue('Use my uploaded files');
    const destination = await id(); expect(destination).not.toBe(source);
    const destinationPaths = paths(destination);
    expect(readFileSync(destinationPaths[0], 'utf8')).toBe('UPLOAD_NATIVE_SENTINEL');
    expect(readFileSync(destinationPaths[1])).toEqual(png);
    // Release the source controller without abandoning the destination editor.
    // The actual TUI adapter cold/unload operation shares the native owner.
    await page.getByRole('button', { name: `Close view ${source}` }).click();
    await expect(page.getByRole('button', { name: `Open ${source}`, exact: true })).toContainText('detached');
    const observer = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    try {
      const attached = await observer.client.call('session/attach', { session_id: source }, 'attached');
      await observer.client.call('session/unload', { target: attached.target }, 'unloaded');
    } finally { await observer.shutdown(); }
    await page.getByLabel(`Actions ${source}`, { exact: true }).click();
    await page.getByRole('button', { name: `Delete ${source}`, exact: true }).click();
    await page.getByRole('button', { name: 'Confirm delete', exact: true }).click();
    await expect(page.getByRole('alert')).toContainText('"status": "deleted"');
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
    await page.getByRole('button', { name: 'Unload runtime', exact: true }).click();
    await expect(page.locator('.session-toolbar small')).toContainText('unloaded');
    await page.getByLabel(`Actions ${destination}`, { exact: true }).click();
    await page.getByRole('button', { name: `Delete ${destination}`, exact: true }).click();
    await page.getByRole('button', { name: 'Confirm delete', exact: true }).click();
    await expect(page.getByRole('alert')).toContainText('"status": "deleted"');
    expect(existsSync(root(destination))).toBe(false);
    expect(errors).toEqual([]); passed = true;
  } finally { await page.close(); await fixture.stop(passed); }
});
