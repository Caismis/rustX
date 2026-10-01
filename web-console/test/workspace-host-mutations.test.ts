// @vitest-environment node
import { afterEach, expect, it, vi } from 'vitest';
import { createServer } from 'node:http';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { LocalWorkspaceHost } from '../host/workspaces';
import { workspaceHandler } from '../host/http';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { WorkspaceHostError, type WorkspaceAuthorityScope } from '../src/workspaces/host';
import { HttpWorkspaceHost } from '../src/workspaces/http-host';

const directories: string[] = [];
afterEach(() => { vi.unstubAllGlobals(); directories.splice(0).forEach(path => rmSync(path, { recursive: true, force: true })); });
function fixture(endpoint = 'ws://localhost:8080/') {
  const directory = mkdtempSync(join(tmpdir(), 'workspace-mutations-')); directories.push(directory);
  const roots = ['a', 'b'].map(id => { const cwd = join(directory, id); mkdirSync(cwd); return { id, cwd, displayName: id }; });
  const config = { endpoint, picker: true, roots, metadataFile: join(directory, 'registrations.json') };
  return { config, host: new LocalWorkspaceHost(config) };
}
function metadata(file: string) {
  const { ino, mtimeNs, ctimeNs } = statSync(file, { bigint: true });
  return { contents: readFileSync(file, 'utf8'), ino, mtimeNs, ctimeNs };
}
function gate() { let resolve!: () => void; const promise = new Promise<void>(done => { resolve = done; }); return { promise, resolve }; }

it('a gated A-scoped HTTP remove cannot write replacement B with the same persistent Workspace ID', async () => {
  const f = fixture(), authority = new WorkspaceAuthority(f.host), observation = await authority.observe();
  const id = observation.catalog.workspaces[0].id;
  let handler = workspaceHandler(f.host);
  const arrived = gate(), execute = gate();
  const server = createServer(async (request, response) => { arrived.resolve(); await execute.promise; await handler(request, response); });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('Missing Host address');
  const origin = `http://127.0.0.1:${address.port}`;
  vi.stubGlobal('location', { href: origin, origin });
  try {
    // Browser preflight still sees A. The actual HTTP write is held before dispatch.
    expect(observation.current()).toBe(true);
    const request = new HttpWorkspaceHost().removeWorkspace(observation.scope, id).then(() => undefined, error => error);
    await arrived.promise;
    const replacement = new LocalWorkspaceHost(f.config), baseline = await replacement.listWorkspaces();
    expect(baseline.authorityId).not.toBe(observation.scope.authorityId);
    expect(baseline.workspaces.map(row => row.id)).toContain(id);
    handler = workspaceHandler(replacement);
    const before = metadata(f.config.metadataFile);
    expect(observation.current()).toBe(true); // No browser observation can fence the server write.
    execute.resolve();
    const failure = await request;
    // Check actual side effects first, so ignoring the Host fence demonstrably removes B's W.
    expect((await replacement.listWorkspaces()).workspaces).toEqual(baseline.workspaces);
    expect(metadata(f.config.metadataFile)).toEqual(before); // Includes inode: no atomic file rewrite.
    expect(failure).toBeInstanceOf(WorkspaceHostError);
    expect(failure).toMatchObject({ kind: 'authority_replaced', uncertain: false });
    await new HttpWorkspaceHost().removeWorkspace({ authorityId: baseline.authorityId, endpoint: baseline.endpoint }, id);
    expect((await replacement.listWorkspaces()).workspaces.map(row => row.id)).not.toContain(id);
    expect(readFileSync(f.config.metadataFile, 'utf8')).not.toContain(id);
  } finally {
    execute.resolve(); server.closeAllConnections();
    await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  }
});

it.each(['adopt', 'rename', 'reorder', 'remove'] as const)('%s shares the Host scope precondition and current scope commits', async method => {
  const f = fixture(), scope = await f.host.listWorkspaces(), id = scope.workspaces[0].id;
  if (method === 'adopt') await f.host.removeWorkspace(scope, id);
  const replacement = new LocalWorkspaceHost(f.config), baseline = await replacement.listWorkspaces();
  const mutation = (expected: WorkspaceAuthorityScope) => {
    switch (method) {
      case 'adopt': return replacement.adoptWorkspace(expected, 'a');
      case 'rename': return replacement.renameWorkspace(expected, id, 'renamed');
      case 'reorder': return replacement.reorderWorkspace(expected, id);
      case 'remove': return replacement.removeWorkspace(expected, id);
    }
  };
  const before = metadata(f.config.metadataFile);
  const failure = await mutation(scope).then(() => undefined, error => error);
  expect((await replacement.listWorkspaces()).workspaces).toEqual(baseline.workspaces);
  expect(metadata(f.config.metadataFile)).toEqual(before);
  expect(failure).toMatchObject({ kind: 'authority_replaced', uncertain: false });
  await mutation(baseline);
  expect((await replacement.listWorkspaces()).workspaces).not.toEqual(baseline.workspaces);
});

it.each(['different', 'equivalent'] as const)('mutation scope compares %s normalized endpoints', async kind => {
  const f = fixture('ws://LOCALHOST:80'), baseline = await f.host.listWorkspaces();
  const scope = { authorityId: baseline.authorityId, endpoint: kind === 'different' ? 'ws://localhost:81/' : 'ws://localhost/./' };
  const before = metadata(f.config.metadataFile), id = baseline.workspaces[0].id;
  if (kind === 'different') {
    await expect(f.host.removeWorkspace(scope, id)).rejects.toMatchObject({ kind: 'authority_replaced', uncertain: false });
    expect((await f.host.listWorkspaces()).workspaces).toEqual(baseline.workspaces);
    expect(metadata(f.config.metadataFile)).toEqual(before);
  } else {
    await f.host.removeWorkspace(scope, id);
    expect((await f.host.listWorkspaces()).workspaces.map(row => row.id)).not.toContain(id);
  }
});

it.each([
  ['adopt', { location: 'a' }], ['rename', { id: 'W', displayName: 'renamed' }],
  ['reorder', { id: 'W' }], ['remove', { id: 'W' }],
] as const)('HTTP %s refuses missing or malformed scope before calling the Host', async (method, params) => {
  const f = fixture(), call = vi.spyOn(f.host, `${method}Workspace`);
  const server = createServer(workspaceHandler(f.host));
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('Missing Host address');
  const before = metadata(f.config.metadataFile);
  try {
    for (const scope of [undefined, null, [], {}, { authorityId: '', endpoint: f.config.endpoint }, { authorityId: 'A', endpoint: 'invalid' }]) {
      const response = await fetch(`http://127.0.0.1:${address.port}/product-host/${method}`, {
        method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ ...params, scope }),
      });
      expect(response.status).toBe(400);
      expect((await response.json()).message).toContain('Expected Product Host scope');
    }
    expect(call).not.toHaveBeenCalled(); expect(metadata(f.config.metadataFile)).toEqual(before);
  } finally { server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve())); }
});
