// @vitest-environment node
import { afterEach, expect, it } from 'vitest';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { LocalWorkspaceHost } from '../host/workspaces';
import { startWorkspaceHost } from './e2e/workspace-host';
const directories: string[] = [];
afterEach(() => { directories.splice(0).forEach(path => rmSync(path, { force: true, recursive: true })); });
function fixture(picker = true, endpoint = 'ws://127.0.0.1:8080/') {
  const directory = mkdtempSync(join(tmpdir(), 'workspace-host-')); directories.push(directory);
  const a = join(directory, 'A'), b = join(directory, 'B'); mkdirSync(a); mkdirSync(b);
  const config = { endpoint, picker, metadataFile: join(directory, 'registrations.json'), roots: [{ id: 'a', cwd: a, displayName: 'Alpha' }, { id: 'b', cwd: b, displayName: 'Beta' }] };
  return { config, a, b, host: new LocalWorkspaceHost(config), endpoint, directory };
}
it('Host metadata groups exact native cwd without owning Session state; unregister/restart cannot recreate registrations', async () => {
  const { host, config, a, b, directory, endpoint } = fixture();
  const [alpha, beta] = (await host.listWorkspaces()).workspaces;
  const sessions = Object.freeze([{ id: 'cold', cwd: a }, { id: 'other', cwd: b }]);
  const before = JSON.stringify(sessions);
  symlinkSync(a, join(directory, 'alias'));
  expect(await host.groupSessions([a, b, join(directory, 'alias'), a + '-outside'], endpoint)).toEqual([alpha.id, beta.id, alpha.id, null]);
  await host.renameWorkspace(alpha.id, 'Renamed'); await host.reorderWorkspace(beta.id, alpha.id);
  expect((await host.listWorkspaces()).workspaces.map(row => row.displayName)).toEqual(['Beta', 'Renamed']);
  expect(await host.resolveWorkspace(alpha.id, endpoint)).toEqual({ cwd: a });
  await host.removeWorkspace(alpha.id);
  expect(await host.groupSessions(sessions.map(row => row.cwd), endpoint)).toEqual([null, beta.id]);
  expect(JSON.stringify(sessions)).toBe(before);
  expect((await new LocalWorkspaceHost(config).listWorkspaces()).workspaces.map(row => row.id)).toEqual([beta.id]);
  expect(readFileSync(config.metadataFile, 'utf8')).not.toMatch(/cwd|session|trust|config/i);
  await host.adoptWorkspace('a'); expect((await host.listWorkspaces()).workspaces).toHaveLength(2);
});
it('only configured opaque handles authorize locations; no path fallback, disabled picker, or cross-process route', async () => {
  const one = fixture(false), two = fixture(true, 'ws://127.0.0.1:8081/');
  const [a] = (await one.host.listWorkspaces()).workspaces;
  expect((await one.host.listWorkspaces()).picker.kind).toBe('unavailable');
  await expect(one.host.adoptWorkspace('a')).rejects.toThrow('unavailable');
  await expect(two.host.adoptWorkspace(one.a)).rejects.toThrow('unavailable');
  await expect(two.host.resolveWorkspace(a.id, two.endpoint)).rejects.toThrow('Unknown');
  await expect(one.host.resolveWorkspace(a.id, two.endpoint)).rejects.toThrow('different rustX process');
  await expect(one.host.groupSessions(Array(33).fill(one.a), one.endpoint)).rejects.toThrow('bounded');
});
it('HTTP carrier refuses path adoption and cross-origin mutations', async () => {
  const f = fixture(); const service = await startWorkspaceHost(f.config);
  try {
    const response = await fetch(`${service.url}/product-host/adopt`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ location: '/arbitrary/path' }) });
    expect(response.status).toBe(400);
    const foreign = await fetch(`${service.url}/product-host/list`, { method: 'POST', headers: { 'Content-Type': 'application/json', Origin: 'https://foreign.example' }, body: '{}' });
    expect(foreign.status).toBe(400);
  } finally { await service.stop(); }
});
