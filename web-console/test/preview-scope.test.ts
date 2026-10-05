import { afterEach, expect, it } from 'vitest';
import { PreviewWorkspaceOwner, samePreviewScope } from '../src/app/preview-workspace';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { Server, snapshot, TOKEN } from './fixture';

const dispose: (() => void)[] = [];
afterEach(() => dispose.splice(0).forEach(stop => stop()));
async function fixture() {
  const server = new Server(); await server.attached('A');
  let authorityId = 'host-A', displayName = 'Workspace A';
  const host = { ...server.workspaceHost, listWorkspaces: async () => {
    const catalog = await server.workspaceHost.listWorkspaces();
    return { ...catalog, authorityId, workspaces: catalog.workspaces.map(row => ({ ...row, displayName })) };
  } };
  const authority = new WorkspaceAuthority(host); await authority.observe();
  const owner = new PreviewWorkspaceOwner(server.client, host, authority), stop = owner.start();
  owner.selectSession('A');
  const artifact = { source: { kind: 'artifact' as const, id: 'same-exact-artifact' }, name: 'same.txt', image: false };
  const occurrence = owner.openPreview(artifact)!;
  owner.updateView(occurrence, { bodyScrollTop: 120, wrap: false });
  const lease = owner.getSnapshot().leases.get(occurrence)!;
  dispose.push(() => { stop(); server.client.disconnect(); });
  return { server, authority, owner, artifact, occurrence, lease,
    display: async () => { displayName = 'Renamed display only'; await authority.observe(); },
    replaceHost: async () => { authorityId = 'host-B'; await authority.observe(); },
  };
}

it('compatibility compares exact runtime, authority, Session and target coordinates without object identity', async () => {
  const f = await fixture(), scope = f.owner.getSnapshot().workspace!.scope;
  expect(samePreviewScope(scope, structuredClone(scope))).toBe(true);
  for (const changed of [
    { ...scope, sessionId: 'B' }, { ...scope, generation: scope.generation + 1 },
    { ...scope, authorityRevision: (scope.authorityRevision ?? 0) + 1 }, { ...scope, hostRevision: scope.hostRevision + 1 },
    { ...scope, target: { ...scope.target, conversation_id: 'replacement-conversation' } },
    { ...scope, target: { ...scope.target, attachment_id: 'replacement-attachment' } },
  ]) expect(samePreviewScope(scope, changed)).toBe(false);
});

it('ordinary Host display and same-target native snapshot refresh keep the same logical occurrence and lease', async () => {
  const f = await fixture(), scope = f.owner.getSnapshot().workspace!.scope;
  await f.display(); await f.server.client.refresh('A');
  expect(f.owner.getSnapshot().workspace!.scope).toEqual(scope);
  expect(f.owner.getSnapshot().workspace!.tabs[0]).toMatchObject({ id: f.occurrence, view: { bodyScrollTop: 120, wrap: false } });
  expect(f.owner.getSnapshot().leases.get(f.occurrence)).toBe(f.lease); expect(f.lease.signal.aborted).toBe(false);
  expect(f.owner.openPreview({ ...f.artifact, source: structuredClone(f.artifact.source) })).toBe(f.occurrence);
});

it.each(['Host', 'runtime', 'native authority', 'Conversation'] as const)('%s replacement retires the same source occurrence before any replacement can open', async replacement => {
  const f = await fixture();
  if (replacement === 'Host') await f.replaceHost();
  if (replacement === 'runtime') {
    f.server.client.disconnect();
    expect(f.lease.signal.aborted).toBe(true); expect(f.owner.getSnapshot().workspace).toBeUndefined();
    await f.server.connect(); await f.server.client.attach('A');
  }
  if (replacement === 'native authority') {
    await f.server.client.connect('ws://127.0.0.1:8089/', TOKEN, 'replace-authority');
    await f.server.client.attach('A');
  }
  if (replacement === 'Conversation') {
    f.server.nodeSnapshots.set('other', { ...snapshot('A'), conversation_id: 'replacement-conversation' });
    await f.server.client.switchNode('A', 'other'); await f.server.client.attach('A');
    expect(f.server.client.target('A').conversation_id).toBe('replacement-conversation');
  }
  expect(f.lease.signal.aborted).toBe(true); expect(f.owner.getSnapshot().leases.size).toBe(0);
  expect(f.owner.getSnapshot().workspace).toBeUndefined();
  const reopened = f.owner.openPreview(f.artifact)!;
  expect(reopened).toBeGreaterThan(f.occurrence); expect(f.owner.getSnapshot().workspace!.tabs[0].view).toEqual({});
  f.owner.updateView(f.occurrence, { bodyScrollTop: 999 });
  expect(f.owner.getSnapshot().workspace!.tabs[0].view).toEqual({});
});
