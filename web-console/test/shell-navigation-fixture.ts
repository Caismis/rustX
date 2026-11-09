import { afterEach } from 'vitest';
import { createActor } from 'xstate';
import { shellNavigationMachine, committedRouteCurrent } from '../src/app/navigation/machine';
import { settingsNavigationMachine, createOwnerLookup } from '../src/app/settings/machines/navigation';
import { WorkspaceSessionAdmission } from '../src/workspaces/session-admission';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import type { DraftPreparations } from '../src/app/new-conversation/preparation';
import type { AppServerClient } from '../src/client/app-server';
import { workflowSurfaceMachine } from '../src/app/workflows/machines/surface';
import { createWorkflowPort } from '../src/app/workflows/port';
import { Server } from './fixture';
const owners: (() => void)[] = [];
afterEach(() => owners.splice(0).forEach(stop => stop()));
/** Drive genuine route replacement for non-React control/admission tests. */
export function routeFixture(client: AppServerClient, host?: Server['workspaceHost'], drafts?: DraftPreparations) {
  const settings = createActor(settingsNavigationMachine, { input: { lookup: createOwnerLookup(host ?? new Server().workspaceHost, client) } }).start();
  const workspaceHost = host ?? new Server().workspaceHost;
  const workspaces = new WorkspaceAuthority(workspaceHost);
  const workflows = createActor(workflowSurfaceMachine, { input: { ports: scope => createWorkflowPort({ client, host: () => workspaceHost, hostAuthority: workspaces.capture(), scope }) } }).start();
  const release = host ? client.setSessionAdmission(new WorkspaceSessionAdmission(workspaces, client).admit) : undefined;
  const actor = createActor(shellNavigationMachine, { input: { client, settings, drafts, host: workspaceHost, workspaces, workflows, route: { kind: 'session', sessionId: 'A' } } }).start();
  const subscription = actor.subscribe(() => client.recheckAdmissions());
  let destination = 0;
  const stop = () => { subscription.unsubscribe(); release?.(); actor.stop(); settings.stop(); workflows.stop(); }; owners.push(stop);
  return { actor, settings, workflows, workspaces, capture: () => committedRouteCurrent(actor), invalidate: () => actor.send({ type: 'DRAFT', workspaceId: String(++destination) }), stop };
}
