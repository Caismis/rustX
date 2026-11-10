import type { ClientView } from '../../client/contracts';
import { sameEndpoint } from '../../workspaces/endpoint';
import type { ShellNavigationContext, ShellRoute } from './machine';

export type SavedRoute = Exclude<ShellRoute, { kind: 'session-draft' | 'settings' }>;
export const navigationPreferencesKey = 'rustx-console-view-v3';
export interface NavigationPreferences { endpoint?: string; openViews: string[]; route: SavedRoute }

/** These are presentation hints. Native admission and Workspace authorization
 * still establish all authority after startup. */
function readRoute(value: unknown): SavedRoute | undefined {
  if (!value || typeof value !== 'object' || !('kind' in value)) return;
  if (value.kind === 'workflows' || value.kind === 'schedules' || value.kind === 'subagents' || value.kind === 'mcp' || value.kind === 'skills') return { kind: value.kind };
  if (value.kind === 'session' && 'sessionId' in value && typeof value.sessionId === 'string') return { kind: 'session', sessionId: value.sessionId };
  if (value.kind === 'new-conversation') {
    if (!('workspaceId' in value)) return { kind: 'new-conversation' };
    if (typeof value.workspaceId === 'string') return { kind: 'new-conversation', workspaceId: value.workspaceId };
  }
}

export function readNavigationPreferences(): NavigationPreferences {
  try {
    const value = JSON.parse(localStorage.getItem(navigationPreferencesKey) ?? 'null');
    const route = readRoute(value?.route);
    if (typeof value?.endpoint === 'string' && Array.isArray(value.openViews) && route) return {
      endpoint: value.endpoint,
      openViews: [...new Set<string>(value.openViews.filter((id: unknown) => typeof id === 'string'))].slice(0, 32),
      route,
    };
  } catch { /* Storage is optional presentation, never recovery evidence. */ }
  return { openViews: [], route: { kind: 'new-conversation' } };
}

export function resumableViews(ids: readonly string[], state: Pick<ClientView, 'views'>) {
  return ids.filter(id => state.views[id]?.attachmentIntent !== 'released' && !state.views[id]?.deleting);
}

export function restoreRoute(preferences: NavigationPreferences, state: Pick<ClientView, 'endpoint' | 'views'>): SavedRoute | undefined {
  if (!sameEndpoint(preferences.endpoint, state.endpoint)) return;
  const route = preferences.route;
  return route.kind !== 'session' || resumableViews(preferences.openViews, state).includes(route.sessionId)
    ? route : { kind: 'new-conversation' };
}

/** A startup selection remains a presentation hint through transient admission
 * failure. Other cold selections never persist an uncommitted Session or draft. */
export function saveRoute(context: Pick<ShellNavigationContext, 'route' | 'home' | 'draftSession' | 'pending' | 'intentRevision'>, openViews: readonly string[]): SavedRoute {
  const { draftSession, pending, intentRevision } = context;
  // Settings is configuration of the retained Home owner, not a replacement
  // startup destination. Its eligible Home selection survives a refresh.
  const route = context.route.kind === 'settings' ? context.home : context.route;
  if (route.kind === 'session-draft' && draftSession?.sessionId === route.sessionId
    && draftSession.intent.destination.kind === 'restore-session'
    && draftSession.intent.destination.sessionId === route.sessionId
    && draftSession.intentRevision === intentRevision
    // Revalidating this retained Home does not replace its startup selection.
    && (!pending || pending === draftSession.intent || pending.destination.kind === 'home' && pending.destination.route === route)
    && openViews.includes(route.sessionId)) {
    return { kind: 'session', sessionId: route.sessionId };
  }
  return route.kind === 'session-draft' || route.kind === 'session' && !openViews.includes(route.sessionId)
    ? { kind: 'new-conversation' } : route;
}
