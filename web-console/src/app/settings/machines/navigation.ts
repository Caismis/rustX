import { assign, fromPromise, setup } from 'xstate';
import type { AppServerClient } from '../../../client/app-server';
import type { ProductHostWorkspaces } from '../../../workspaces/host';
import { userSettingsTarget, workspaceSettingsTarget, type SettingsTarget } from '../projection';

export type SettingsSection = 'overview' | 'connection';

/** What an owning-Workspace lookup can truthfully answer. There is deliberately
 * no error channel: a lookup that completes under a retired authority answers
 * `retired`, which commits nothing and reports nothing. */
export type OwnerResolution =
  | { kind: 'resolved'; id: string; displayName: string }
  | { kind: 'unregistered'; directory: string }
  | { kind: 'failed'; message: string }
  | { kind: 'retired' };

/** The Product Host catalog lookup of one native source owner, with the App
 * Server authority it is allowed to commit under captured at its start. */
export type OwnerLookup = (directory: string) => Promise<OwnerResolution>;

export function createOwnerLookup(host: ProductHostWorkspaces, client: AppServerClient): OwnerLookup {
  return async directory => {
    const authority = client.getSnapshot().authorityRevision;
    const current = () => client.getSnapshot().authorityRevision === authority;
    try {
      const catalog = await host.listWorkspaces();
      if (!current()) return { kind: 'retired' };
      const row = catalog.workspaces.find(workspace => workspace.displayPath === directory);
      return row ? { kind: 'resolved', id: row.id, displayName: row.displayName } : { kind: 'unregistered', directory };
    } catch (cause) {
      return current() ? { kind: 'failed', message: String(cause) } : { kind: 'retired' };
    }
  };
}

export interface SettingsNavigationContext {
  lookup: OwnerLookup;
  /** The open Settings surface, or `undefined` while Settings is closed. */
  section?: SettingsSection;
  /** The owner an opened Settings instance is permanently bound to. */
  target: SettingsTarget;
  error: string;
}

export type SettingsNavigationEvent =
  | { type: 'OPEN'; target: SettingsTarget; section?: SettingsSection }
  | { type: 'OPEN.CONNECTION' }
  | { type: 'OPEN.OWNER'; directory: string }
  | { type: 'CLOSE' }
  | { type: 'RETIRE' }
  | { type: 'DISMISS' };

/** Top-level Settings navigation.
 *
 * The invariant this machine exists to make structural is:
 *
 * > Asynchronous owning-Workspace lookup is preparation, not ongoing authority
 * > to navigate.
 *
 * Every navigation-affecting decision — open User Settings, open a Workspace's
 * Settings, open the owning Workspace's Settings, open Connection, close, or
 * authority replacement — re-enters `idle`, which *stops* the lookup actor. A
 * stale lookup then has no completion path at all, so neither its success nor
 * its failure can overwrite a newer decision, reopen a closed dialog or publish
 * an obsolete error. Both are silent because neither runs. */
export const settingsNavigationMachine = setup({
  types: {
    context: {} as SettingsNavigationContext,
    events: {} as SettingsNavigationEvent,
    input: {} as { lookup: OwnerLookup },
  },
  actors: {
    resolveOwner: fromPromise(({ input }: { input: { lookup: OwnerLookup; directory: string } }) => input.lookup(input.directory)),
  },
  guards: {
    ownerResolved: ({ event }) => (event as unknown as { output: OwnerResolution }).output.kind === 'resolved',
    ownerUnregistered: ({ event }) => (event as unknown as { output: OwnerResolution }).output.kind === 'unregistered',
    ownerFailed: ({ event }) => (event as unknown as { output: OwnerResolution }).output.kind === 'failed',
  },
  actions: {
    openTarget: assign({
      target: ({ context, event }) => event.type === 'OPEN' ? event.target : context.target,
      section: ({ context, event }) => event.type === 'OPEN' ? event.section ?? 'overview' : context.section,
      error: () => '',
    }),
    // Connection is a client-owned surface and never changes the configuration
    // owner an editor is bound to.
    openConnection: assign({ section: () => 'connection' as const, error: () => '' }),
    close: assign({ section: () => undefined, error: () => '' }),
    clearError: assign({ error: () => '' }),
    openResolvedOwner: assign({
      target: ({ context, event }) => {
        const output = (event as unknown as { output: OwnerResolution }).output;
        return output.kind === 'resolved' ? workspaceSettingsTarget(output.id, output.displayName) : context.target;
      },
      section: () => 'overview' as const,
      error: () => '',
    }),
    reportOwnerLookup: assign({
      error: ({ context, event }) => {
        const output = (event as unknown as { output: OwnerResolution }).output;
        return output.kind === 'unregistered' ? `The owning Workspace ${output.directory} is not registered by this Product Host.`
          : output.kind === 'failed' ? output.message : context.error;
      },
    }),
  },
}).createMachine({
  id: 'settingsNavigation',
  context: ({ input }) => ({ lookup: input.lookup, target: userSettingsTarget, error: '' }),
  initial: 'idle',
  states: {
    idle: {},
    /** Preparation only. Leaving this state cancels the lookup. */
    resolvingOwner: {
      invoke: {
        src: 'resolveOwner',
        input: ({ context, event }) => ({ lookup: context.lookup, directory: (event as Extract<SettingsNavigationEvent, { type: 'OPEN.OWNER' }>).directory }),
        onDone: [
          { guard: 'ownerResolved', target: 'idle', actions: 'openResolvedOwner' },
          { guard: 'ownerUnregistered', target: 'idle', actions: 'reportOwnerLookup' },
          { guard: 'ownerFailed', target: 'idle', actions: 'reportOwnerLookup' },
          // A retired authority commits nothing and reports nothing.
          { target: 'idle' },
        ],
      },
    },
  },
  on: {
    OPEN: { target: '.idle', reenter: true, actions: 'openTarget' },
    'OPEN.CONNECTION': { target: '.idle', reenter: true, actions: 'openConnection' },
    'OPEN.OWNER': { target: '.resolvingOwner', reenter: true, actions: 'clearError' },
    CLOSE: { target: '.idle', reenter: true, actions: 'close' },
    // Authority replacement retires the lookup without reinterpreting the
    // presentation decision the user already made.
    RETIRE: { target: '.idle', reenter: true, actions: 'clearError' },
    DISMISS: { actions: 'clearError' },
  },
});
