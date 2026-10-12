import { afterEach } from 'vitest';
import { createActor } from 'xstate';
import { shellNavigationMachine, committedRouteCurrent } from '../src/app/navigation/machine';
import { settingsNavigationMachine, createOwnerLookup } from '../src/app/settings/machines/navigation';
import { WorkspaceSessionAdmission } from '../src/workspaces/session-admission';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import type { AppServerClient } from '../src/client/app-server';
import { Server } from './fixture';
const owners: (() => void)[] = [];
afterEach(() => owners.splice(0).forEach(stop => stop()));
/** Drive genuine route replacement for non-React control/admission tests. */
export function routeFixture(client: AppServerClient, host?: Server['workspaceHost']) {
  const settings = createActor(settingsNavigationMachine, { input: { lookup: createOwnerLookup(host ?? new Server().workspaceHost, client) } }).start();
  const release = host ? client.setSessionAdmission(new WorkspaceSessionAdmission(new WorkspaceAuthority(host), client).admit) : undefined;
  const actor = createActor(shellNavigationMachine, { input: { client, settings, route: { kind: 'session', sessionId: 'A' } } }).start();
  const subscription = actor.subscribe(() => client.recheckAdmissions());
  let destination = 0;
  const stop = () => { subscription.unsubscribe(); release?.(); actor.stop(); settings.stop(); }; owners.push(stop);
  return { actor, settings, capture: () => committedRouteCurrent(actor), invalidate: () => actor.send({ type: 'DRAFT', workspaceId: String(++destination) }), stop };
}
