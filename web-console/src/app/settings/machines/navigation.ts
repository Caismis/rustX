import { assign, fromPromise, setup } from 'xstate';
import type { AppServerClient } from '../../../client/app-server';
import type { ProductHostWorkspaces } from '../../../workspaces/host';
import {
  settingsLanding, settingsTargetKey, userSettingsTarget, workspaceSettingsTarget,
  type SettingsFocus, type SettingsPage, type SettingsTarget,
} from '../projection';

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
  /** The open primary product page, or `undefined` while Settings is closed. */
  page?: SettingsPage;
  /** The secondary focus of each primary page — a Provider, a Model, an
   * extension resource, or the Advanced Connection sub-surface.
   *
   * Focus is kept per page rather than as one current value, so leaving a
   * detail for another page and coming back restores exactly the detail the
   * user left. It is presentation navigation and nothing else: no draft, no
   * CAS base and no mutation identity lives here, so discarding a focus can
   * never discard editing intent. */
  focus: Partial<Record<SettingsPage, SettingsFocus>>;
  /** The owner an opened Settings instance is permanently bound to. */
  target: SettingsTarget;
  error: string;
}

export type SettingsNavigationEvent =
  | { type: 'OPEN'; target: SettingsTarget }
  | { type: 'OPEN.CONNECTION' }
  | { type: 'OPEN.OWNER'; directory: string }
  /** Select another primary page of the open dialog, keeping its target and
   * every page's own focus. */
  | { type: 'SELECT'; page: SettingsPage }
  /** Focus a detail of the currently displayed page, or leave it for the list. */
  | { type: 'FOCUS'; focus?: SettingsFocus }
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
 * Settings, open the owning Workspace's Settings, open Connection, select a
 * page of the open dialog, focus a detail, close, or authority replacement —
 * re-enters `idle`, which *stops* the lookup actor. A stale lookup then has no
 * completion path at all, so neither its success nor its failure can overwrite
 * a newer decision, reopen a closed dialog or publish an obsolete error. Both
 * are silent because neither runs.
 *
 * It is also the one owner of *which* surface is displayed. A primary page and
 * the detail focused inside it are both navigation facts of this machine, so
 * the rendered surface can never disagree with the latest decision, and a
 * detail view can never be reached without a page that admits it. */
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
    /** A page can be selected only inside an open Settings dialog. */
    settingsOpen: ({ context }) => context.page !== undefined,
  },
  actions: {
    openTarget: assign({
      target: ({ context, event }) => event.type === 'OPEN' ? event.target : context.target,
      // The landing page is derived from the pages that owner authorizes, so a
      // constrained Workspace surface never lands on a page it does not have.
      page: ({ context, event }) => settingsLanding(event.type === 'OPEN' ? event.target : context.target),
      // A different owner's details are not this owner's. Focus never migrates
      // across targets, exactly as a draft never does.
      focus: () => ({}),
      error: () => '',
    }),
    // Connection is a client-owned surface. It is a sub-surface of Advanced
    // rather than a primary page, and it belongs to the global client: a
    // constrained Workspace surface never exposes it, so deciding to open it
    // is also a decision to author globally.
    openConnection: assign({
      target: () => userSettingsTarget,
      page: () => 'advanced' as const,
      // The global client's other page focus survives; a Workspace's does not
      // migrate into the global surface.
      focus: ({ context }) => ({
        ...(settingsTargetKey(context.target) === settingsTargetKey(userSettingsTarget) ? context.focus : {}),
        advanced: { kind: 'connection' as const },
      }),
      error: () => '',
    }),
    select: assign({
      page: ({ context, event }) => event.type === 'SELECT' ? event.page : context.page,
      error: () => '',
    }),
    focus: assign({
      focus: ({ context, event }) => {
        if (event.type !== 'FOCUS' || context.page === undefined) return context.focus;
        const next = { ...context.focus };
        if (event.focus) next[context.page] = event.focus; else delete next[context.page];
        return next;
      },
    }),
    close: assign({ page: () => undefined, error: () => '' }),
    clearError: assign({ error: () => '' }),
    openResolvedOwner: assign({
      target: ({ context, event }) => {
        const output = (event as unknown as { output: OwnerResolution }).output;
        return output.kind === 'resolved' ? workspaceSettingsTarget(output.id, output.displayName) : context.target;
      },
      page: ({ context, event }) => {
        const output = (event as unknown as { output: OwnerResolution }).output;
        return settingsLanding(output.kind === 'resolved' ? workspaceSettingsTarget(output.id, output.displayName) : context.target);
      },
      focus: () => ({}),
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
  context: ({ input }) => ({ lookup: input.lookup, target: userSettingsTarget, focus: {}, error: '' }),
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
    SELECT: { guard: 'settingsOpen', target: '.idle', reenter: true, actions: 'select' },
    FOCUS: { guard: 'settingsOpen', target: '.idle', reenter: true, actions: 'focus' },
    CLOSE: { target: '.idle', reenter: true, actions: 'close' },
    // Authority replacement retires the lookup without reinterpreting the
    // presentation decision the user already made.
    RETIRE: { target: '.idle', reenter: true, actions: 'clearError' },
    DISMISS: { actions: 'clearError' },
  },
});
