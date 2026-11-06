/* Copyright (c) 2026 DeepSeek. MIT. Adapted recentWorkspace selection; see PROVENANCE.md. */
import type { ClientView } from '../client/contracts';
import type { AssociationSnapshot } from './associations';
import { sameEndpoint } from './endpoint';

/** Current Session first, then the explicit preference, then native activity.
 * Host order breaks ties when no selection or activity exists.
 * Registration/classification are display hints. Send still reauthorizes. */
export function preferredWorkspace(associations: AssociationSnapshot, state: Pick<ClientView, 'connection' | 'endpoint' | 'sessions' | 'views'>, sessionId?: string, remembered?: string): string | undefined {
  const catalog = associations.catalog;
  if (state.connection !== 'connected' || associations.status !== 'ready' || !catalog || !sameEndpoint(catalog.endpoint, state.endpoint)) return;
  const registered = new Set(catalog.workspaces.map(workspace => workspace.id));
  const workspaceOf = (id: string, cwd: string | undefined) => {
    const entry = associations.entries.get(id), workspace = entry?.confirmed?.workspaceId;
    return cwd !== undefined && entry?.cwd === cwd && entry.status === 'ready' && workspace && registered.has(workspace) ? workspace : undefined;
  };
  if (sessionId) {
    const summary = state.sessions.find(session => session.id === sessionId) ?? state.views[sessionId]?.summary;
    const current = workspaceOf(sessionId, summary?.cwd ?? state.views[sessionId]?.settings?.cwd);
    if (current) return current;
  }
  if (remembered && registered.has(remembered)) return remembered;
  if (catalog.preferredWorkspace && registered.has(catalog.preferredWorkspace)) return catalog.preferredWorkspace;
  const activity = new Map<string, number>();
  const listed = new Set(state.sessions.map(session => session.id));
  for (const [id, entry] of associations.entries) {
    const workspace = workspaceOf(id, entry.cwd);
    if (!listed.has(id) && workspace && entry.updatedAt !== undefined) activity.set(workspace, Math.max(activity.get(workspace) ?? -Infinity, entry.updatedAt));
  }
  for (const session of state.sessions) {
    const workspace = workspaceOf(session.id, session.cwd), updated = Date.parse(session.updated_at);
    if (workspace && Number.isFinite(updated)) activity.set(workspace, Math.max(activity.get(workspace) ?? -Infinity, updated));
  }
  let selected: string | undefined, selectedTime = -Infinity;
  for (const workspace of catalog.workspaces) {
    const latest = activity.get(workspace.id) ?? -Infinity;
    if (selected === undefined || latest > selectedTime) { selected = workspace.id; selectedTime = latest; }
  }
  return selected;
}
