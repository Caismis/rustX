import { test, expect } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { createHash } from 'node:crypto';
import { AppServerHost } from '../../../tui/src/app-server/host.ts';
import { startDogfood } from './dogfood-server';
function gate() { let release!: () => void; const promise = new Promise<void>(r => { release = r; }); return { promise, release }; }

test('real immutable Artifact crosses private v2 derivation and rejects late attachment publication', async ({ page }, info) => {
  const fixture = await startDogfood('web_artifact_document');
  const nativeSocket = globalThis.WebSocket;
  const carrier: { protocol: string; payload: any }[] = [];
  // Observe, never synthesize, the Host's private socket traffic.
  globalThis.WebSocket = class extends nativeSocket {
    private readonly carrierProtocol: string;
    constructor(url: string | URL, protocols?: string | string[]) {
      super(url, protocols); this.carrierProtocol = Array.isArray(protocols) ? protocols[0] : '';
    }
    send(data: Parameters<WebSocket['send']>[0]) {
      if (this.carrierProtocol === 'rustx.product-host.file-read.v3') carrier.push({ protocol: this.carrierProtocol, payload: JSON.parse(String(data)) });
      super.send(data);
    }
  };
  let passed = false, remote: AppServerHost | undefined;
  const responseReady = gate(), publish = gate(); let hold = false;
  try {
    remote = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    const catalog = await fixture.workspaceHost.host.listWorkspaces();
    const context = await fixture.workspaceHost.host.resolveWorkspace(catalog.workspaces[0].id, fixture.endpoint);
    const { session } = await remote.createSession(context);
    const conversation = session.active_conversation_id;
    const files = readdirSync(join(fixture.directory, 'runtime'), { recursive: true }).map(String);
    const database = files.find(path => path.includes(conversation) && path.endsWith('conversation.sqlite'));
    expect(database).toBeDefined();
    const source = resolve('test/fixtures/documents/sample.xlsx'), original = readFileSync(source);
    const id = execFileSync(resolve('../target/debug/examples/document_artifact_fixture'), [conversation, dirname(join(fixture.directory, 'runtime', database!)), source], { encoding: 'utf8' }).trim();
    await remote.shutdown(); remote = undefined;
    const requests: any[] = [];
    await page.route('**/product-host/*', async route => {
      const body = route.request().postDataJSON();
      const response = await fetch(fixture.workspaceHostUrl + new URL(route.request().url()).pathname, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
      const text = await response.text();
      if (route.request().url().endsWith('/document-preview')) {
        requests.push(body.request);
        if (hold) { responseReady.release(); await publish.promise; }
      }
      await route.fulfill({ status: response.status, headers: Object.fromEntries(response.headers), body: text });
    });
    await page.goto(`http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/artifact-document.html`);
    await page.evaluate(async config => { await (window as any).artifactDocument.setup(...config); }, [fixture.endpoint, fixture.token, session.id, id]);
    await page.getByRole('button', { name: 'Open Artifact', exact: true }).click();
    await expect(page.getByRole('cell', { name: '99', exact: true })).toBeVisible();
    expect(requests[0].source).toEqual({ kind: 'artifact', artifact_id: id });
    expect(requests[0].digest).toBe(createHash('sha256').update(original).digest('hex'));
    expect(JSON.stringify(requests)).not.toContain(JSON.parse(readFileSync(fixture.hostConfigFile, 'utf8')).productHostToken);
    expect(carrier).toHaveLength(2); // Original Artifact load uses the ordinary native owner; derivation rereads twice.
    for (const entry of carrier) {
      expect(entry.payload.source).toEqual({ kind: 'artifact', artifact_id: id });
      expect(entry.payload.roots).toEqual([]);
      expect(JSON.stringify(entry.payload)).not.toContain(fixture.token);
      expect(JSON.stringify(entry.payload)).not.toContain(fixture.directory);
    }
    const target = await page.evaluate(() => (window as any).artifactDocument.target());
    // Same-spelled missing id remains native owner rejection, not path access.
    const scope = await fixture.workspaceHost.host.listWorkspaces();
    await expect(fixture.workspaceHost.host.previewDocument(scope, { ...requests[0], digest: '0'.repeat(64) })).rejects.toThrow('source_changed');
    await expect(fixture.workspaceHost.host.previewDocument(scope, { ...requests[0], source: { kind: 'artifact', artifact_id: 'artifact_999' } })).rejects.toThrow();
    const downloading = page.waitForEvent('download'); await page.getByRole('button', { name: 'Download artifact', exact: true }).click();
    const download = await downloading; expect(download.suggestedFilename()).toBe('报告 original.xlsx');
    const destination = info.outputPath('original.xlsx'); await download.saveAs(destination); expect(readFileSync(destination)).toEqual(original);
    await page.getByRole('button', { name: 'Close preview', exact: true }).click(); hold = true;
    await page.getByRole('button', { name: 'Open Artifact', exact: true }).click(); await responseReady.promise;
    await page.evaluate(async () => { await (window as any).artifactDocument.replaceAttachment(); });
    publish.release(); await expect(page.getByRole('alert')).toBeVisible();
    await expect(page.getByRole('cell', { name: '99', exact: true })).toHaveCount(0);
    await expect(fixture.workspaceHost.host.previewDocument(scope, { ...requests[0], target })).rejects.toThrow();
    expect((await fixture.control('requests')).requests).toHaveLength(0);
    await page.evaluate(async () => { await (window as any).artifactDocument.dispose(); }); passed = true;
  } finally { publish.release(); globalThis.WebSocket = nativeSocket; await remote?.shutdown(); await fixture.stop(passed); }
});
