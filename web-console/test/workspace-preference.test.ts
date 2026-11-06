import { expect, it } from 'vitest';
import { preferredWorkspace } from '../src/workspaces/preferred';
import type { AssociationSnapshot } from '../src/workspaces/associations';
import type { ClientView } from '../src/client/contracts';
import { Server, endpoint } from './fixture';
function fixture() {
  const server = new Server();
  const state: ClientView = { ...server.client.getSnapshot(), connection: 'connected', endpoint,
    sessions: [{ ...server.summary('A'), updated_at: '2026-11-01T00:00:00Z' }, { ...server.summary('B'), updated_at: '2026-11-06T00:00:00Z' }] };
  const associations: AssociationSnapshot = { status: 'ready', catalog: { authorityId: 'host', endpoint, picker: { kind: 'unavailable', reason: '' },
    workspaces: ['A', 'B'].map(id => ({ id, location: id, displayName: id, displayPath: `/workspace/${id}` })) },
    entries: new Map(state.sessions.map(session => [session.id, { cwd: session.cwd, status: 'ready', confirmed: { workspaceId: session.id } }])) };
  return { state, associations };
}
it('uses latest native activity rather than catalog, pinned or Workspace order', () => {
  const f = fixture(); expect(preferredWorkspace(f.associations, f.state)).toBe('B');
  f.state.sessions = [...f.state.sessions].reverse(); expect(preferredWorkspace(f.associations, f.state)).toBe('B');
});
it('inherits the current registered Workspace before a more recent one', () => {
  const f = fixture(); expect(preferredWorkspace(f.associations, f.state, 'A')).toBe('A');
});
it('keeps Host order for equal activity and for registered Workspaces with no known Sessions', () => {
  const f = fixture(); f.state.sessions = f.state.sessions.map(session => ({ ...session, updated_at: '2026-11-01T00:00:00Z' })).reverse();
  expect(preferredWorkspace(f.associations, f.state)).toBe('A');
  f.state.sessions = []; expect(preferredWorkspace(f.associations, f.state)).toBe('A');
});
it('ignores removed registrations and classification for a different native cwd', () => {
  const f = fixture(); f.state.sessions = f.state.sessions.map(session => session.id === 'B' ? { ...session, cwd: '/moved/B' } : session);
  expect(preferredWorkspace(f.associations, f.state, 'B')).toBe('A');
  f.associations.catalog!.workspaces = []; expect(preferredWorkspace(f.associations, f.state)).toBeUndefined();
});
it.each(['pending', 'refreshing', 'unavailable', 'disconnected'] as const)('waits while Workspace classification is %s', status => {
  const f = fixture(); expect(preferredWorkspace({ ...f.associations, status }, f.state)).toBeUndefined();
});
it('never selects a Workspace from a disconnected or different endpoint', () => {
  const f = fixture(); expect(preferredWorkspace(f.associations, { ...f.state, connection: 'disconnected' })).toBeUndefined();
  expect(preferredWorkspace(f.associations, { ...f.state, endpoint: 'ws://different:8080' })).toBeUndefined();
});
