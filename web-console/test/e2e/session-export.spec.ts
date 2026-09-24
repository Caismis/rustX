import { openEmptySession } from './shell-actions';
import { test, expect } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { join } from 'node:path';
import { AppServerHost } from '../../../tui/src/app-server/host.ts';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, showInspector, expectSettled } from './shell-actions';
import { wireProbe } from './wire-probe';
function logicalArchive(path: string) {
  // Test-only decoder. Production has no import/restore path.
  return JSON.parse(execFileSync('python3', ['-c', `import zipfile,json,sys,base64
with zipfile.ZipFile(sys.argv[1]) as z:
 print(json.dumps({n:base64.b64encode(z.read(n)).decode() for n in sorted(z.namelist())}))`, path], { encoding: 'utf8' }));
}
test('browser native download and remote TUI consume the same authenticated native archive', async ({ page }) => {
  const fixture = await startDogfood('web_session_archive');
  const wire = await wireProbe(page);
  let remote: AppServerHost | undefined;
  let passed = false;
  try {
    await routeWorkspaceHost(page, fixture);
    await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Archive this Session');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByText('Archive fixture settled.', { exact: true })).toBeVisible();
    await expectSettled(page); await showInspector(page);
    const id = JSON.parse(await page.getByLabel('Native diagnostic JSON').innerText()).SessionId as string;
    const downloadStarted = page.waitForEvent('download');
    await page.getByRole('button', { name: 'Session actions', exact: true }).click();
    await page.screenshot({ path: test.info().outputPath('session-export-menu.png') });
    await page.getByRole('menuitem', { name: 'Export', exact: true }).click();
    const download = await downloadStarted;
    expect(download.suggestedFilename()).toBe(`rustx-session-${id}.zip`);
    const browserPath = join(fixture.directory, 'browser.zip');
    await download.saveAs(browserPath);
    expect(await download.failure()).toBeNull();
    const prepare = wire.requests.filter(r => r.method === 'session/exportPrepare');
    expect(prepare).toHaveLength(1);
    expect(prepare[0].params).toEqual({ session_id: id });
    expect(wire.requests.filter(r => r.method === 'artifact/read')).toHaveLength(0);
    expect(download.url()).not.toContain(fixture.token);
    expect((await fetch(download.url())).status).toBe(401);
    expect((await fetch(download.url().replace(/[^/]+$/, 'a'.repeat(43)))).status).toBe(401);
    remote = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    const calls: { method: string; params: unknown }[] = [];
    const call = remote.client.call.bind(remote.client);
    remote.client.call = ((method: string, params: unknown, result: unknown) => {
      calls.push({ method, params }); return (call as Function)(method, params, result);
    }) as typeof remote.client.call;
    const localPath = join(fixture.directory, 'tui-local.zip');
    expect(await remote.exportSession(id, localPath)).toBe(localPath);
    expect(calls).toEqual([{ method: 'session/exportPrepare', params: { session_id: id } }]);
    const web = logicalArchive(browserPath), tui = logicalArchive(localPath);
    expect(tui).toEqual(web);
    const manifest = JSON.parse(Buffer.from(web['manifest.json'], 'base64').toString());
    expect(manifest.format).toBe('rustx-session-archive/v2');
    expect(manifest.schemas).toEqual({ journal: 2, messages: 1, surface: 1, requests: 2, generations: 1, publication_audits: 1, inherited_responses: 1 });
    expect(Object.keys(web).some(key => key.endsWith('/generations.jsonl'))).toBe(true);
    const logical = Object.values(web).map(value => Buffer.from(value as string, 'base64').toString()).join('\n');
    expect(logical).toContain('Archive fixture settled.');
    expect(logical).not.toContain(fixture.token);
    expect(logical).not.toContain('fake-provider-only');
    passed = true;
  } finally { await remote?.shutdown(); await fixture.stop(passed); }
});
