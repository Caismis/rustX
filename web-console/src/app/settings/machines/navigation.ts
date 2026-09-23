import { assign, fromPromise, setup, type ActorRefFrom } from 'xstate';
import type { AppServerClient } from '../../../client/app-server';
import type { ProductHostWorkspaces } from '../../../workspaces/host';
import {
  settingsTargetKey, userSettingsTarget, workspaceSettingsTarget,
  type ExtensionFamily, type SettingsTarget,
} from '../projection';

/** The six product pages Settings is organized around.
 *
 * They are user tasks, not native semantic-unit names: a user configures a
 * Provider, chooses a default model, writes guidance, grants Tool access or
 * inspects an extension without ever meeting `root_model`, `native_tools` or
 * `source_tools`. Native identities, exact CAS writes and application facts are
 * unchanged underneath — only what the browser groups them into is new. */
export type SettingsPage = 'general' | 'models' | 'agent' | 'tools' | 'extensions' | 'advanced';
const globalSettingsPages: readonly SettingsPage[] = ['general', 'models', 'agent', 'tools', 'extensions', 'advanced'];
export function settingsPageLabel(page: SettingsPage): string {
  return page === 'general' ? 'General'
    : page === 'models' ? 'Models'
      : page === 'agent' ? 'Agent'
        : page === 'tools' ? 'Tools & Permissions'
          : page === 'extensions' ? 'Extensions' : 'Advanced';
}

/** The secondary focus of each primary page, by page.
 *
 * This is presentation navigation with exactly one owner, this machine. It
 * carries identities only: no draft, no CAS base and no mutation lives here, so
 * focusing, leaving and refocusing a detail can never create, migrate or
 * discard editing intent. A page with no secondary surface admits no focus at
 * all, and a focus is typed by the page that owns it, so no page can be handed
 * another page's detail. */
export interface PageFocus {
  general: never;
  /** `provider` on a Model records which Provider detail it was opened from,
   * so leaving the Model returns to that Provider rather than the bare list. */
  models: { kind: 'provider'; id: string } | { kind: 'model'; id: string; provider?: string };
  agent: never;
  tools: never;
  extensions: { kind: 'extension'; family: ExtensionFamily; name: string };
  /** Connection is the client-owned sub-surface of Advanced. */
  advanced: { kind: 'connection' };
}
export type SettingsFocus = PageFocus[SettingsPage];
export type FocusMap = { [P in SettingsPage]?: PageFocus[P] };

/** The pages one exact owner authorizes.
 *
 * Workspace Settings is a constrained override surface, not a second copy of
 * global Settings: General holds client-owned preferences that no native source
 * authors at all, so a Workspace has no General page rather than an empty one. */
export function settingsPages(target: SettingsTarget): readonly SettingsPage[] {
  return target.kind === 'user' ? globalSettingsPages : globalSettingsPages.filter(page => page !== 'general');
}
/** The page an owner's Settings opens at: the first page it authorizes. */
export function settingsLanding(target: SettingsTarget): SettingsPage {
  return settingsPages(target)[0];
}
/** Whether one owner's Settings may display this page. */
export function admitsPage(target: SettingsTarget, page: SettingsPage): boolean {
  return settingsPages(target).includes(page);
}
/** Whether one owner's page may focus this detail.
 *
 * This is the whole secondary-navigation capability matrix, decided once:
 *
 * - Models focuses a Provider or a Model;
 * - Extensions focuses one extension resource;
 * - Advanced focuses Connection, and only for the User target — Connection is
 *   client-owned global configuration, and a Workspace is a constrained
 *   source-authoring surface that never exposes it;
 * - General, Agent and Tools & Permissions have no secondary focus.
 *
 * A focus that belongs to another page is never this page's focus. */
export function admitsFocus(target: SettingsTarget, page: SettingsPage, focus: SettingsFocus): boolean {
  if (!admitsPage(target, page)) return false;
  switch (focus.kind) {
    case 'provider': case 'model': return page === 'models';
    case 'extension': return page === 'extensions';
    case 'connection': return page === 'advanced' && target.kind === 'user';
  }
}
/** The Connection focus, which Advanced admits only for the User target. */
export const connectionFocus: PageFocus['advanced'] = { kind: 'connection' };

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
  /** The open primary product page, or `undefined` while Settings is closed.
   * Always a page `target` authorizes. */
  page?: SettingsPage;
  /** The secondary focus of each primary page — a Provider, a Model, an
   * extension resource, or the Advanced Connection sub-surface.
   *
   * Focus is kept per page rather than as one current value, so leaving a
   * detail for another page and coming back restores exactly the detail the
   * user left. Every entry is one `admitsFocus` accepts for `target`. It is
   * presentation navigation and nothing else: no draft, no CAS base and no
   * mutation identity lives here, so discarding a focus can never discard
   * editing intent. */
  focus: FocusMap;
  /** The owner an opened Settings instance is permanently bound to. */
  target: SettingsTarget;
  error: string;
}

export type SettingsNavigationEvent =
  | { type: 'OPEN'; target: SettingsTarget }
  | { type: 'OPEN.CONNECTION' }
  | { type: 'OPEN.OWNER'; directory: string }
  /** Select another primary page of the open dialog, keeping its target and
   * every page's own focus. A page the target does not authorize is refused. */
  | { type: 'SELECT'; page: SettingsPage }
  /** Focus a detail of the currently displayed page, or leave it for the list.
   * A focus the displayed page and target do not admit is refused. */
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
 * It is also the one owner of *which* surface is displayed, and of whether that
 * surface is legal for its owner:
 *
 * > A Settings target can never enter a navigation state that the target does
 * > not authorize.
 *
 * `page ∈ settingsPages(target)` and every focus satisfies `admitsFocus`, at
 * all times. `SELECT` and `FOCUS` are guarded by exactly those capabilities, so
 * an illegal request is refused rather than admitted and repaired later; every
 * transition that changes the target also lands on that target's landing page
 * and drops focus that belonged to another owner. The presentation therefore
 * renders machine state as it is and never reinterprets it. */
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
    /** A page can be selected only inside an open Settings dialog, and only if
     * the dialog's owner authorizes it. */
    pageAdmitted: ({ context, event }) => context.page !== undefined
      && event.type === 'SELECT' && admitsPage(context.target, event.page),
    /** A detail can be focused only on the open page that owns it, and only if
     * the dialog's owner authorizes it there. Leaving a detail is always legal. */
    focusAdmitted: ({ context, event }) => context.page !== undefined && event.type === 'FOCUS'
      && (event.focus === undefined || admitsFocus(context.target, context.page, event.focus)),
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
        advanced: connectionFocus,
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
        const next: Partial<Record<SettingsPage, SettingsFocus>> = { ...context.focus };
        // `focusAdmitted` has proved this focus belongs to exactly this page.
        if (event.focus) next[context.page] = event.focus; else delete next[context.page];
        return next as FocusMap;
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
    SELECT: { guard: 'pageAdmitted', target: '.idle', reenter: true, actions: 'select' },
    FOCUS: { guard: 'focusAdmitted', target: '.idle', reenter: true, actions: 'focus' },
    CLOSE: { target: '.idle', reenter: true, actions: 'close' },
    // Authority replacement retires the lookup without reinterpreting the
    // presentation decision the user already made.
    RETIRE: { target: '.idle', reenter: true, actions: 'clearError' },
    DISMISS: { actions: 'clearError' },
  },
});

export type SettingsNavigationActor = ActorRefFrom<typeof settingsNavigationMachine>;
