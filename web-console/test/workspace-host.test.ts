import { WorkspaceAuthority } from '../src/workspaces/authority';
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
  mkdirSync(join(a, 'descendant'));
  expect(await host.classifyLocations([join(a, 'descendant')], endpoint)).toEqual([{ authorized: false, reason: 'denied' }]);
  expect(await host.classifyLocations([a, b, join(directory, 'alias'), a + '-outside'], endpoint)).toEqual([{ authorized: true, workspaceId: alpha.id }, { authorized: true, workspaceId: beta.id }, { authorized: true, workspaceId: alpha.id }, { authorized: false, reason: 'unavailable' }]);
  await host.renameWorkspace(alpha.id, 'Renamed'); await host.reorderWorkspace(beta.id, alpha.id);
  expect((await host.listWorkspaces()).workspaces.map(row => row.displayName)).toEqual(['Beta', 'Renamed']);
  expect(await host.resolveWorkspace(alpha.id, endpoint)).toEqual({ cwd: a });
  await host.removeWorkspace(alpha.id);
  expect(await host.classifyLocations(sessions.map(row => row.cwd), endpoint)).toEqual([{ authorized: true }, { authorized: true, workspaceId: beta.id }]);
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
  await expect(one.host.classifyLocations(Array(33).fill(one.a), one.endpoint)).rejects.toThrow('bounded');
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

it('canonical endpoint identity accepts URL equivalence and rejects different hosts/ports', async () => {
  const f = fixture(true, 'ws://LOCALHOST:80'), id = (await f.host.listWorkspaces()).workspaces[0].id;
  expect(await f.host.resolveWorkspace(id, 'ws://localhost/')).toEqual({ cwd: f.a });
  expect(await f.host.classifyLocations([f.a], 'ws://localhost:80/./')).toEqual([{ authorized: true, workspaceId: id }]);
  await expect(f.host.resolveWorkspace(id, 'ws://localhost:81/')).rejects.toThrow('different rustX process');
  await expect(f.host.resolveWorkspace(id, 'ws://example.test/')).rejects.toThrow('different rustX process');
});

it('unavailable roots fail fresh admission, unregister preserves it, and symlink/replacement revoke it', async () => {
  const { host, a, b, config, endpoint } = fixture();
  const [workspace] = (await host.listWorkspaces()).workspaces;
  const { WorkspaceSessionNavigation } = await import('../src/workspaces/navigation');
  const { NavigationEpoch } = await import('../src/client/navigation');
  const client = { getSnapshot: () => ({ generation: 1, endpoint }), request: async () => ({ settings: { cwd: a } }) } as unknown as import('../src/client/app-server').AppServerClient;
  const admission = new WorkspaceSessionNavigation(new WorkspaceAuthority(host), client, new NavigationEpoch());
  expect(await (await admission.admit('native-session', () => true) as import('../src/client/app-server').OperationAdmission).validate(new AbortController().signal)).toBe(true);
  rmSync(a, { recursive: true });
  expect(await host.classifyLocations([a], endpoint)).toEqual([{ authorized: false, reason: 'unavailable' }]);
  await expect(admission.admit('native-session', () => true)).rejects.toThrow('not authorized');
  mkdirSync(a); await host.removeWorkspace(workspace.id);
  expect(await host.classifyLocations([a], endpoint)).toEqual([{ authorized: true }]);
  expect(await (await admission.admit('native-session', () => true) as import('../src/client/app-server').OperationAdmission).validate(new AbortController().signal)).toBe(true);
  rmSync(a, { recursive: true }); symlinkSync(b, a);
  expect(await host.classifyLocations([a], endpoint)).toEqual([{ authorized: false, reason: 'denied' }]);
  await expect(admission.admit('native-session', () => true)).rejects.toThrow('not authorized');
  rmSync(a); mkdirSync(a);
  const replacement = new LocalWorkspaceHost({ ...config, roots: config.roots.filter(root => root.cwd !== a) });
  expect(await replacement.classifyLocations([a], endpoint)).toEqual([{ authorized: false, reason: 'denied' }]);
  const revoked = new WorkspaceSessionNavigation(new WorkspaceAuthority(replacement), client, new NavigationEpoch());
  await expect(revoked.admit('native-session', () => true)).rejects.toThrow('not authorized');
  expect((await replacement.listWorkspaces()).authorityId).not.toBe((await host.listWorkspaces()).authorityId);
  await expect(replacement.classifyLocations([b], endpoint, (await host.listWorkspaces()).authorityId)).rejects.toThrow('replaced');
});

it('the HTTP carrier preserves definitive Host replacement observations', async () => {
  const f = fixture(); const service = await startWorkspaceHost(f.config);
  try {
    const response = await fetch(`${service.url}/product-host/classify`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ cwds: [f.a], endpoint: f.endpoint, authorityId: 'retired-host' }) });
    expect(response.status).toBe(400);
    expect((await response.json()).kind).toBe('authority_replaced');
  } finally { await service.stop(); }
});

it('unregister and re-adopt changes registration identity under the same Host authority', async () => {
  const { host, a, endpoint } = fixture();
  const before = await host.listWorkspaces(), old = before.workspaces[0];
  await host.removeWorkspace(old.id);
  expect(await host.classifyLocations([a], endpoint, before.authorityId)).toEqual([{ authorized: true }]);
  await host.adoptWorkspace('a');
  const after = await host.listWorkspaces(), replacement = after.workspaces.find(row => row.location === 'a')!;
  expect(after.authorityId).toBe(before.authorityId);
  expect(replacement.id).not.toBe(old.id);
  expect(await host.classifyLocations([a], endpoint, before.authorityId)).toEqual([{ authorized: true, workspaceId: replacement.id }]);
});
