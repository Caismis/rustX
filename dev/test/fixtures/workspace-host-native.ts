import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { LocalWorkspaceHost } from '../../../web-console/host/workspaces.ts';
import { workspaceHandler } from '../../../web-console/host/http.ts';
import { WorkspaceHostError, validateLocations } from '../../../web-console/src/workspaces/host.ts';

assert.equal(process.env.NODE_OPTIONS, undefined);
assert.equal(process.env.NODE_PATH, undefined);
assert.deepEqual(process.execArgv, []);
const directory = mkdtempSync(join(tmpdir(), 'rustx-native-workspace-'));
const server = createServer();
try {
  const cwd = join(directory, 'workspace');
  mkdirSync(cwd);
  const endpoint = 'ws://127.0.0.1:8080/';
  const host = new LocalWorkspaceHost({ endpoint, picker: true, metadataFile: join(directory, 'registrations.json'), roots: [{ id: 'root', cwd, displayName: 'Workspace' }] });
  const catalog = await host.listWorkspaces();
  const rows = await host.classifyLocations([cwd, directory], endpoint, catalog.authorityId);
  validateLocations(rows, 2);
  assert.deepEqual(rows, [{ authorized: true, workspaceId: catalog.workspaces[0].id }, { authorized: false, reason: 'denied' }]);
  await assert.rejects(host.classifyLocations([cwd], endpoint, 'retired-host'), error => error instanceof WorkspaceHostError && error.kind === 'authority_replaced');
  server.on('request', workspaceHandler(host));
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  const response = await fetch(`http://127.0.0.1:${address.port}/product-host/classify`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ cwds: [cwd], endpoint, authorityId: 'retired-host' }),
  });
  assert.equal(response.status, 400);
  const failure = await response.json();
  assert.equal(failure.kind, 'authority_replaced');
  assert.equal(failure.uncertain, false);
  console.log('native Workspace boundary passed');
} finally {
  server.closeAllConnections();
  if (server.listening) await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  rmSync(directory, { recursive: true, force: true });
}
