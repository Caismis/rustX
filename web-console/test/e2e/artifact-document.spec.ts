import { test, expect } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { AppServerHost } from '../../../tui/src/app-server/host.ts';
import { fileResponse } from '../../src/workspaces/file-response';
import { FILE_PREVIEW_LIMITS } from '../../shared/file-preview';
import type { DocumentResult } from '../../shared/documents';
import { startDogfood } from './dogfood-server';
import type { WorkspaceHostResponse } from './workspace-host';
function gate() { let release!: () => void; const promise = new Promise<void>(r => { release = r; }); return { promise, release }; }

test('real immutable Artifact crosses private binary derivation and rejects late attachment publication', async ({ page }, info) => {
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
      if (this.carrierProtocol === 'rustx.product-host.file-read.v5') carrier.push({ protocol: this.carrierProtocol, payload: JSON.parse(String(data)) });
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
    const source = resolve('test/fixtures/documents/sample.docx'), original = readFileSync(source);
    const id = execFileSync(resolve(dirname(process.env.RUSTX_BINARY ?? '../target/debug/rustx'), 'examples/document_artifact_fixture'), [conversation, dirname(join(fixture.directory, 'runtime', database!)), source], { encoding: 'utf8' }).trim();
    await remote.shutdown(); remote = undefined;
    const requests: any[] = [];
    const documentResponses = new Map<string, WorkspaceHostResponse>();
    await page.route('**/product-host/*', async route => {
      const body = route.request().postDataJSON();
      const response = await fetch(fixture.workspaceHostUrl + new URL(route.request().url()).pathname, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
      const buffered = { status: response.status, headers: Object.fromEntries(response.headers), body: Buffer.from(await response.arrayBuffer()) };
      if (route.request().url().endsWith('/document-preview')) {
        requests.push(body.request);
        documentResponses.set(body.operationId, buffered);
        if (hold) { responseReady.release(); await publish.promise; }
      }
      await route.fulfill(buffered);
    });
    await page.goto(`http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/artifact-document.html`);
    await page.evaluate(async config => { await (window as any).artifactDocument.setup(...config); }, [fixture.endpoint, fixture.token, session.id, id]);
    const derivation = page.waitForResponse(response => response.request().method() === 'POST'
      && response.url().endsWith('/product-host/document-preview') && response.request().postDataJSON()?.request?.source?.artifact_id === id);
    await page.getByRole('button', { name: 'Open Artifact', exact: true }).click();
    // Physical Office conversion settles before the PDF's DOM render budget.
    // Admission headers alone do not prove a complete, successful derivation.
    const response = await derivation, request = response.request().postDataJSON();
    expect(request.operationId).toMatch(/^[a-f0-9-]{36}$/);
    expect(response.headers()['x-rustx-document-operation']).toBe(request.operationId);
    expect(response.status()).toBe(200);
    // The proxy has consumed the complete Host response before fulfilling the
    // browser request. Inspect those same bytes: Chromium's completion event
    // for a fulfilled route can remain pending after the PDF has rendered.
    const buffered = documentResponses.get(request.operationId);
    expect(buffered).toBeDefined();
    expect(buffered!.status).toBe(response.status());
    expect(buffered!.headers['x-rustx-document-operation']).toBe(request.operationId);
    expect(buffered!.headers['content-type']).toBe(response.headers()['content-type']);
    const terminal = await fileResponse(new Response(Uint8Array.from(buffered!.body), {
      status: buffered!.status, headers: buffered!.headers,
    }), FILE_PREVIEW_LIMITS.convertedPdfBytes) as { ok: boolean; value: DocumentResult };
    expect(terminal.ok).toBe(true);
    expect(terminal.value.digest).toBe(createHash('sha256').update(original).digest('hex'));
    expect(terminal.value.preview.kind).toBe('pdf');
    expect(terminal.value.preview.data).toBeInstanceOf(Uint8Array);
    documentResponses.delete(request.operationId);
    await expect(page.locator('[data-pdf-page="1"] .textLayer')).toContainText('rustX');
    expect(requests[0].source).toEqual({ kind: 'artifact', artifact_id: id });
    expect(requests[0]).not.toHaveProperty('digest');
    expect(JSON.stringify(requests)).not.toContain(JSON.parse(readFileSync(fixture.hostConfigFile, 'utf8')).productHostToken);
    expect(carrier).toHaveLength(3); // Host reads the original and reauthorizes before and after conversion.
    for (const entry of carrier) {
      expect(entry.payload.source).toEqual({ kind: 'artifact', artifact_id: id });
      expect(entry.payload.roots).toEqual([]);
      expect(JSON.stringify(entry.payload)).not.toContain(fixture.token);
      expect(JSON.stringify(entry.payload)).not.toContain(fixture.directory);
    }
    const target = await page.evaluate(() => (window as any).artifactDocument.target());
    // Same-spelled missing id remains native owner rejection, not path access.
    const scope = await fixture.workspaceHost.host.listWorkspaces();
    await expect(fixture.workspaceHost.host.previewDocument(scope, { ...requests[0], digest: '0'.repeat(64) })).rejects.toThrow('Invalid document coordinates');
    await expect(fixture.workspaceHost.host.previewDocument(scope, { ...requests[0], source: { kind: 'artifact', artifact_id: 'artifact_999' } })).rejects.toThrow();
    const downloading = page.waitForEvent('download'); await page.getByRole('button', { name: 'Download artifact', exact: true }).click();
    const download = await downloading; expect(download.suggestedFilename()).toBe('报告 original.docx');
    const destination = info.outputPath('original.docx'); await download.saveAs(destination); expect(readFileSync(destination)).toEqual(original);
    await page.getByRole('button', { name: 'Close preview', exact: true }).click(); hold = true;
    await page.getByRole('button', { name: 'Open Artifact', exact: true }).click(); await responseReady.promise;
    await page.evaluate(async () => { await (window as any).artifactDocument.replaceAttachment(); });
    publish.release(); await expect(page.getByRole('alert')).toBeVisible();
    await expect(page.locator('[data-pdf-preview]')).toHaveCount(0);
    await expect(fixture.workspaceHost.host.previewDocument(scope, { ...requests[0], target })).rejects.toThrow();
    expect((await fixture.control('requests')).requests).toHaveLength(0);
    await page.evaluate(async () => { await (window as any).artifactDocument.dispose(); }); passed = true;
  } finally { publish.release(); globalThis.WebSocket = nativeSocket; await remote?.shutdown(); await fixture.stop(passed); }
});
