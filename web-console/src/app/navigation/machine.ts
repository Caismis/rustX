import { actionError } from '../../bindings/action-error';
import { assign, fromCallback, fromPromise, setup, type ActorRefFrom } from 'xstate';
import type { AppServerClient } from '../../client/app-server';
import type { CompletedResponseView, UserInputBlock } from '../../../../protocol/app-server/v75';
import type { Observation } from '../../client/session-lifecycle/port';
import { RpcFailure } from '../../client/errors';
import { CommandSession, type HistoricalSelection, type HistoryAction } from '../commands/native';
import type { CommandRequest } from '../commands/CommandPanel';
import type { SettingsNavigationActor, SettingsNavigationEvent } from '../settings/machines/navigation';
import { firstSubmissions } from '../new-conversation/first-submit';
import type { DraftPreparations } from '../new-conversation/preparation';
import { responseRetry, responseRetryReady, type ResponseRetry } from '../agent/response-retry';
import { forkLatestSession } from '../commands/fork-session';
import { attemptOwnerKey } from '../../../../protocol/app-server/attempt-identity';
import type { WorkspaceAuthority } from '../../workspaces/authority';
import type { ProductHostWorkspaces } from '../../workspaces/host';
import { sameEndpoint } from '../../workspaces/endpoint';
import type { WorkflowSurfaceActor } from '../workflows/machines/surface';

class RetiredPreparation extends Error {}
export type ConversationRoute = { kind: 'new-conversation'; workspaceId?: string } | { kind: 'session'; sessionId: string } | { kind: 'session-draft'; sessionId: string };
export interface WorkflowsRoute { kind: 'workflows' }
export interface SchedulesRoute { kind: 'schedules' }
export interface SettingsRoute { kind: 'settings' }
export type ShellRoute = ConversationRoute | WorkflowsRoute | SchedulesRoute | SettingsRoute;
export function isHomeRoute(route: ShellRoute): route is ConversationRoute {
  return route.kind === 'new-conversation' || route.kind === 'session' || route.kind === 'session-draft';
}
function sameEditorDestination(a: ShellRoute, b: ShellRoute) {
  const from = a, to = b;
  return 'sessionId' in from && 'sessionId' in to ? from.sessionId === to.sessionId
    : from.kind === 'new-conversation' && to.kind === 'new-conversation' && from.workspaceId === to.workspaceId;
}
type Destination =
  | { kind: 'home'; route: ConversationRoute; binding: number }
  | { kind: 'creation'; binding: string }
  | { kind: 'session'; sessionId: string; sidebar: boolean; command?: CommandRequest; nodeId?: string; conversationId?: string }
  | { kind: 'restore-session'; sessionId: string; sidebar: boolean; command?: CommandRequest }
  | { kind: 'transition'; sessionId: string; action: HistoryAction; response?: CompletedResponseView; selection?: HistoricalSelection }
  | { kind: 'fork-session'; sessionId: string; sidebar?: boolean }
  | { kind: 'node'; sessionId: string; nodeId: string; conversationId: string };
interface Pending { recovery?: true; destination: Destination; route: ShellRoute; generation: number; authority: number | undefined }
interface Prepared { warning?: string; pending: Pending; route: ShellRoute; observation?: Observation; content?: UserInputBlock[]; replay?: ResponseRetry['replay'] }
interface Input { drafts?: DraftPreparations; client: AppServerClient; settings: SettingsNavigationActor; host: ProductHostWorkspaces; workspaces: WorkspaceAuthority; workflows: WorkflowSurfaceActor; route?: ShellRoute }
type NavigationEvent = Extract<ShellNavigationEvent, { type: 'OPEN' | 'DRAFT' | 'HOME' | 'WORKFLOWS' | 'SCHEDULES' | 'SETTINGS' }>;
interface WorkflowLeave { event: NavigationEvent }
export interface ShellNavigationContext extends Omit<Input, 'route'> {
  // Home keeps its editor and controls across primary pages. Executable
  // conversation transitions establish their own control identity.
  binding: number; control: object; route: ShellRoute; pending?: Pending; error?: string;
  /** Explicit navigation intent, independent of a draft's retained identity. */
  intentRevision: number;
  /** A presentation hint, revalidated before Home is published again. */
  home: ConversationRoute;
  leaving?: WorkflowLeave;
  approvedLeave?: WorkflowLeave;
  retry?: ResponseRetry;
  command?: { request: CommandRequest; route: ShellRoute; generation: number };
  restored?: { conversation: string; content: UserInputBlock[] };
  /** Admission history permits retaining hints, never executable authority. */
  draftSession?: { sessionId: string; intent: Pending; intentRevision: number; wasAdmitted: boolean };
  /** Keep an unscoped New action's origin until its Workspace is classified. */
  draftWorkspaceSession?: string;
  publication?: { sidebar: boolean; intent?: Pending; control: object };
  dismissedSidebar?: Pending;
  generation: number; authority: number | undefined;
}
export type ShellNavigationEvent =
  | { type: 'OPEN'; destination: Destination }
  | { type: 'DRAFT'; workspaceId?: string }
  | { type: 'DRAFT.WORKSPACE'; workspaceId: string }
  | { type: 'CREATION.KNOWN'; pending: Pending; sessionId: string }
  | { type: 'RETRY.OBSERVED'; retry: ResponseRetry }
  | { type: 'CREATE'; binding: string; control: object }
  | { type: 'CREATED'; sessionId: string; control: object; generation: number; pending?: Pending }
  | { type: 'COMMAND'; request: CommandRequest }
  | { type: 'COMMAND.CLOSE'; command: NonNullable<ShellNavigationContext['command']> }
  | { type: 'SETTINGS'; destination?: Extract<SettingsNavigationEvent, { type: 'OPEN' | 'OPEN.CONNECTION' | 'OPEN.OWNER' | 'OPEN.REPAIRS' }> }
  | { type: 'HOME' }
  | { type: 'WORKFLOW.LEFT'; intent: WorkflowLeave }
  | { type: 'WORKFLOW.CANCELLED'; intent: WorkflowLeave }
  | { type: 'WORKFLOWS' }
  | { type: 'SCHEDULES' }
  | { type: 'SIDEBAR.DISMISS' | 'DISMISS.ERROR' }
  | { type: 'CANCEL'; pending?: Pending }
  | { type: 'SESSION.RESUME'; intent: Pending; generation: number; authority: number | undefined }
  | { type: 'AUTHORITY'; generation: number; authority: number | undefined };

function selectedSessionDraft(context: ShellNavigationContext) {
  const draft = context.draftSession, intent = draft?.intent;
  return context.route.kind === 'session-draft' && draft?.sessionId === context.route.sessionId
    && (!context.pending || context.pending === intent)
    && (intent?.destination.kind === 'session' || intent?.destination.kind === 'restore-session')
    && intent.destination.sessionId === context.route.sessionId;
}
function authorityCurrent(client: AppServerClient, pending: Pending) {
  const state = client.getSnapshot();
  return state.connection === 'connected' && state.generation === pending.generation && state.authorityRevision === pending.authority;
}
function sessionRecoveryCurrent(client: AppServerClient, sessionId: string) {
  return client.isAttachmentPreparing(sessionId)
    || client.isAttachmentControlCurrent(sessionId, client.getSnapshot().views[sessionId]?.attachmentObservation);
}
/** A selected Session owns local input before it owns executable controls.
 * Ordinary selection and History Return share this target-bound publication. */
function pendingSessionDraft(context: ShellNavigationContext, pending: Pending, sessionId: string, intentRevision = context.intentRevision) {
  const route = { kind: 'session-draft' as const, sessionId };
  const destination = pending.destination;
  const previous = context.draftSession?.intent.destination;
  const sameTask = destination.kind === 'session' && previous?.kind === 'session'
    && previous.conversationId === destination.conversationId && previous.nodeId === destination.nodeId;
  const replacing = !sameEditorDestination(context.route, route)
    || !sameTask && destination.kind === 'session' && !!destination.conversationId
      && context.client.getSnapshot().views[sessionId]?.target?.conversation_id !== destination.conversationId;
  return { binding: replacing ? context.binding + 1 : context.binding,
    control: replacing ? {} : context.control,
    route, draftSession: { sessionId, intent: pending, intentRevision,
      wasAdmitted: pending.recovery === true && !replacing && context.draftSession?.wasAdmitted === true } };
}
/** Disposable preparation owns publication only. Native calls below are finite
 * lifecycle commands: aborting this actor retires admission, never settlement. */
async function prepare(input: { client: AppServerClient; host: ProductHostWorkspaces; workspaces: WorkspaceAuthority; pending: Pending; parent: { send(event: ShellNavigationEvent): void } }, signal: AbortSignal): Promise<Prepared> {
  const { client, pending } = input, destination = pending.destination;
  const current = () => !signal.aborted && authorityCurrent(client, pending);
  if (destination.kind === 'home') {
    let route = destination.route;
    const submission = route.kind === 'new-conversation' ? firstSubmissions(client).draft(String(destination.binding)) : undefined;
    if (submission?.session) route = { kind: 'session-draft', sessionId: submission.session.id };
    const empty: Prepared = { pending, route: { kind: 'new-conversation' } };
    try {
      if (route.kind === 'new-conversation') {
        const native = client.getSnapshot();
        const workspaceId = route.workspaceId;
        if (workspaceId) {
          const observation = await input.workspaces.observe(signal);
          if (!observation.current() || !native.endpoint || !sameEndpoint(observation.catalog.endpoint, native.endpoint)
            || !observation.catalog.workspaces.some(row => row.id === workspaceId)) return empty;
          const resolved = await input.host.resolveWorkspace(workspaceId, native.endpoint);
          const [location] = await input.workspaces.classify([resolved.cwd], native.endpoint, observation, signal);
          if (!observation.current() || !location.authorized) return empty;
        }
        // Creation may finish while Workspace authorization is suspended.
        // Revalidate that Session against fresh lifecycle state before Home,
        // including deletion that settled during the same authorization wait.
        const created = firstSubmissions(client).draft(String(destination.binding))?.session;
        if (!created) return { pending, route };
        route = { kind: 'session-draft', sessionId: created.id };
      }
      const native = client.getSnapshot();
      const sessionId = route.sessionId;
      const view = native.views[sessionId];
      if (!view || view.deleting || view.deletionRecovery) return empty;
      // Attachment intent is native execution authority, not ownership of the
      // retained local editor. Uncertain settlement can release that intent.
      // A disconnected Home is a local draft/recovery surface, never executable.
      // Reconnect and attachment admission remain exclusively lifecycle-owned.
      if (native.connection !== 'connected') return { pending, route: { kind: 'session-draft', sessionId } };
      const summary = view.summary ?? native.sessions.find(row => row.id === sessionId)
        ?? await client.readSessionSummary(sessionId);
      const cwd = summary?.cwd ?? view.settings?.cwd;
      if (!cwd || !native.endpoint) return empty;
      const observation = await input.workspaces.observe(signal);
      const [location] = await input.workspaces.classify([cwd], native.endpoint, observation, signal);
      const latest = client.getSnapshot().views[sessionId];
      if (!observation.current() || !location.authorized || !latest || latest.deleting || latest.deletionRecovery) return empty;
      const executable = client.isAttachmentControlCurrent(sessionId, latest.attachmentObservation);
      return { pending, route: { kind: executable ? 'session' : 'session-draft', sessionId }, observation: executable ? latest.attachmentObservation : undefined };
    } catch {
      // A remembered identity cannot substitute for fresh authorization.
      return empty;
    }
  }
  if (destination.kind === 'creation') {
    // Observe the native FirstSubmissions owner; this actor never creates,
    // attaches or executes the draft. A Home return can replace publication
    // interest while the original creation and Send continue to settle.
    return await new Promise<Prepared>((resolve, reject) => {
      const inspect = () => {
        const flow = firstSubmissions(client).draft(destination.binding);
        if (flow?.session) input.parent.send({ type: 'CREATION.KNOWN', pending, sessionId: flow.session.id });
        // A known Session outlives recovery or discard of its first input.
        // Neither action creates or attaches that Session again.
        if (flow && !flow.session && ['failed', 'rejected', 'uncertain', 'discarded', 'paused'].includes(flow.phase)) { cleanup(); reject(flow.error ?? new Error('Conversation preparation stopped.')); return; }
        // FirstSubmissions advances past attaching only after its native port
        // has completed the handoff. Do not replace that port's control fence.
        if (flow?.session && flow.phase !== 'attaching') {
          const observation = client.getSnapshot().views[flow.session.id]?.attachmentObservation;
          if (client.isAttachmentControlCurrent(flow.session.id, observation)) {
            cleanup(); resolve({ pending, route: { kind: 'session', sessionId: flow.session.id }, observation });
          }
        }
      };
      const release = firstSubmissions(client).subscribe(inspect);
      // An admitted owner is quiet while native resync can still suspend controls.
      const releaseNative = client.subscribe(inspect);
      const retired = () => { cleanup(); reject(new Error('Conversation publication retired.')); };
      const cleanup = () => { release(); releaseNative(); signal.removeEventListener('abort', retired); };
      signal.addEventListener('abort', retired, { once: true }); inspect();
    });
  }
  let sessionId = destination.sessionId, content: UserInputBlock[] | undefined, replay: ResponseRetry['replay'];
  let readyObservation: Observation | undefined;
  if (destination.kind === 'session' || destination.kind === 'restore-session') {
    try {
      const requestedNode = destination.kind === 'session' ? destination.nodeId : undefined;
      const requestedConversation = destination.kind === 'session' ? destination.conversationId : undefined;
      if (requestedNode && requestedConversation && client.isAttachmentControlCurrent(sessionId, client.getSnapshot().views[sessionId]?.attachmentObservation)
        && client.getSnapshot().views[sessionId]?.target?.conversation_id !== requestedConversation) {
        const scope = new CommandSession(client, sessionId, current);
        if (!await scope.openNode(requestedNode, requestedConversation)) throw new RetiredPreparation();
      }
      if (pending.recovery) {
        // Automatic navigation owns publication only. Lifecycle alone decides
        // whether transport recovery may Open; unresolved effects are not retried.
        await client.waitForAttachment(sessionId, current);
      } else if (destination.kind === 'restore-session' && firstSubmissions(client).session(sessionId) && !['admitted', 'discarded'].includes(firstSubmissions(client).session(sessionId)!.phase)) {
        if (firstSubmissions(client).session(sessionId)!.phase === 'attaching') await client.waitForAttachment(sessionId, current);
        if (!client.isAttachmentControlCurrent(sessionId, client.getSnapshot().views[sessionId]?.attachmentObservation)) return { pending, route: { kind: 'session-draft', sessionId } };
      } else if (!client.isAttachmentControlCurrent(sessionId, client.getSnapshot().views[sessionId]?.attachmentObservation)) {
        // Native attachment evidence admits publication; supplementary metadata
        // settlement continues in Session Lifecycle after this disposable read.
        await new Promise<void>((resolve, reject) => {
          void client.attach(sessionId, requestedNode, current, () => {
            const proof = client.getSnapshot().views[sessionId]?.attachmentObservation;
            if (client.isAttachmentControlCurrent(sessionId, proof)) readyObservation = proof;
            resolve();
          }).then(resolve, reject);
        });
      }
      if (requestedConversation && current() && client.getSnapshot().views[sessionId]?.target?.conversation_id !== requestedConversation) throw new RetiredPreparation();
    } catch (error) {
      if (destination.kind !== 'restore-session' || !(error instanceof RpcFailure) || error.error.data?.kind !== 'unknown_session' || !current()) throw error;
      // Only Native can establish that a saved identity no longer exists.
      // Release this obsolete hint, which owns no runtime claim, and never
      // choose another Session on the reader's behalf.
      await client.release(sessionId);
      return { pending, route: { kind: 'new-conversation' } };
    }
  } else if (destination.kind === 'fork-session') {
    const child = await forkLatestSession(client, sessionId, current);
    if (!child) throw new RetiredPreparation();
    return { pending, warning: child.warning, route: pending.route, observation: pending.route.kind === 'session' ? client.getSnapshot().views[pending.route.sessionId]?.attachmentObservation : undefined };
  } else {
    const scope = new CommandSession(client, sessionId, current);
    if (destination.kind === 'node') {
      if (!await scope.openNode(destination.nodeId, destination.conversationId)) throw new Error('Node navigation retired.');
    } else {
      const selection = destination.selection ?? await scope.responseSelection(destination.response!);
      const result = await scope.transition(destination.action, selection);
      if (!result) throw new Error('Historical transition retired.');
      sessionId = result.session.id; content = result.content; replay = result.replay;
    }
  }
  const observation = readyObservation ?? client.getSnapshot().views[sessionId]?.attachmentObservation;
  // Ready was proved by the native checkpoint/subscription handoff. A later
  // same-claim resync disables controls but does not undo that navigation proof.
  if ((destination.kind === 'session' || destination.kind === 'restore-session') && !readyObservation && !client.isAttachmentControlCurrent(sessionId, observation)) throw new RetiredPreparation();
  return { pending, route: { kind: 'session', sessionId }, observation, content, replay };
}

/** The only top-level route publication owner. Exiting preparing stops only its
 * promise actor; native lifecycle actors keep already dispatched transactions.
 * XState's correlated invoke completion plus the final proof guard publishes
 * executable routes. A cold Session selection immediately owns a separate local
 * draft so its first Send can arm Native before Attach. History/lineage reads
 * still preserve their committed route until preparation completes. */
export const shellNavigationMachine = setup({
  types: { context: {} as ShellNavigationContext, input: {} as Input, events: {} as ShellNavigationEvent },
  actors: { drafts: fromCallback<ShellNavigationEvent, { owner?: DraftPreparations; parent: { getSnapshot(): { context: Pick<ShellNavigationContext, 'home' | 'binding' | 'client'> }; subscribe(listener: () => void): { unsubscribe(): void } } }>(({ input }) => {
    const follow = () => {
      const { home: route, binding, client } = input.parent.getSnapshot().context;
      const submission = route.kind === 'session-draft' ? firstSubmissions(client).session(route.sessionId) : undefined;
      if (submission?.draft.preparation && !['rejected', 'discarded'].includes(submission.phase)) {
        // A creation ACK publishes durable identity before native validated
        // transfer. FirstSubmissions retains this exact binding even if Settings
        // or same-Session selection replaces disposable navigation interest.
        // A Session route may retain a sealed selection, never start a new one.
        input.owner?.follow(submission.binding);
      } else input.owner?.follow(route.kind === 'new-conversation' ? String(binding) : undefined, route.kind === 'new-conversation' ? route.workspaceId : undefined);
    };
    const subscription = input.parent.subscribe(follow); follow();
    return () => subscription.unsubscribe();
  }), authority: fromCallback<ShellNavigationEvent, { client: AppServerClient; parent: { getSnapshot(): { context: Pick<ShellNavigationContext, 'generation' | 'authority' | 'retry' | 'route' | 'draftSession' | 'pending'> } } }>(({ input, sendBack }) => {
    const observe = () => {
      const native = input.client.getSnapshot(), owner = input.parent.getSnapshot().context;
      if (native.generation !== owner.generation || native.authorityRevision !== owner.authority) sendBack({ type: 'AUTHORITY', generation: native.generation, authority: native.authorityRevision });
      else if (native.connection === 'connected' && owner.route.kind === 'session-draft' && owner.draftSession
        && !owner.pending
        && sessionRecoveryCurrent(input.client, owner.draftSession.sessionId)
        && (owner.draftSession.intent.destination.kind === 'session' || owner.draftSession.intent.destination.kind === 'restore-session'))
        sendBack({ type: 'SESSION.RESUME', intent: owner.draftSession.intent, generation: native.generation, authority: native.authorityRevision });
      else if (owner.retry && responseRetryReady(owner.retry, native.views[owner.retry.view.id])) sendBack({ type: 'RETRY.OBSERVED', retry: owner.retry });
    };
    const release = input.client.subscribe(observe); observe(); return release;
  }), prepare: fromPromise<Prepared, { client: AppServerClient; host: ProductHostWorkspaces; workspaces: WorkspaceAuthority; pending: Pending; parent: { send(event: ShellNavigationEvent): void } }>(({ input, signal }) => prepare(input, signal)) },
  guards: {
    workflowLeave: ({ context, event }) => context.route.kind === 'workflows' && context.approvedLeave?.event !== event,
    sameTransition: ({ context, event }) => {
      if (event.type !== 'OPEN') return false;
      const pending = context.pending?.destination, next = event.destination;
      if (pending?.kind === 'fork-session' && next.kind === 'fork-session') return pending.sessionId === next.sessionId;
      return pending?.kind === 'transition' && next.kind === 'transition'
        && pending.sessionId === next.sessionId && pending.action === next.action
        && (pending.response && next.response ? attemptOwnerKey(pending.response.origin) === attemptOwnerKey(next.response.origin) : pending.selection === next.selection);
    },
    prepared: ({ context, event }) => {
      const output = (event as unknown as { output: Prepared }).output;
      const native = context.client.getSnapshot();
      const current = output.pending.destination.kind === 'home'
        ? native.generation === output.pending.generation && native.authorityRevision === output.pending.authority
        : authorityCurrent(context.client, output.pending);
      return output.pending === context.pending && current
        && (output.route.kind !== 'session' || context.client.isAttachmentObservationCurrent(output.route.sessionId, output.observation));
    },
    createdOwner: ({ context, event }) => event.type === 'CREATION.KNOWN' && context.pending === event.pending && context.pending.destination.kind === 'creation',
    resumingCreation: ({ context }) => {
      if (context.route.kind !== 'new-conversation' || context.client.getSnapshot().connection !== 'connected') return false;
      const flow = firstSubmissions(context.client).draft(String(context.binding));
      return !!flow && (flow.phase === 'creating' || !!flow.session);
    },
    failed: ({ context, event }) => !!context.pending && authorityCurrent(context.client, context.pending) && !((event as unknown as { error: unknown }).error instanceof RetiredPreparation),
    attached: ({ context, event }) => {
      if (event.type !== 'OPEN' || event.destination.kind !== 'session' && event.destination.kind !== 'restore-session') return false;
      const destination = event.destination, view = context.client.getSnapshot().views[destination.sessionId];
      return context.client.isAttachmentControlCurrent(destination.sessionId, view?.attachmentObservation)
        && (destination.kind !== 'session' || (!destination.conversationId || destination.conversationId === view?.target?.conversation_id)
          && (!destination.nodeId || destination.nodeId === (view?.nodeId ?? view?.summary?.active_node)));
    },
    cancelling: ({ context, event }) => event.type === 'CANCEL' && (!event.pending || context.pending === event.pending),
    selectedSessionDraft: ({ context }) => selectedSessionDraft(context),
    sidebar: ({ context }) => (context.pending?.destination.kind === 'session' || context.pending?.destination.kind === 'fork-session') && !!context.pending.destination.sidebar,
    drafting: ({ context }) => context.route.kind === 'new-conversation' && (!firstSubmissions(context.client).draft(String(context.binding)) || ['rejected', 'discarded'].includes(firstSubmissions(context.client).draft(String(context.binding))!.phase)),
    draftTargetAvailable: ({ context }) => context.home.kind !== 'new-conversation' || !firstSubmissions(context.client).draft(String(context.binding)) || ['admitted', 'rejected', 'discarded'].includes(firstSubmissions(context.client).draft(String(context.binding))!.phase),
    commandRoute: ({ context }) => context.route.kind === 'session' && context.client.isAttachmentControlCurrent(context.route.sessionId, context.client.getSnapshot().views[context.route.sessionId]?.attachmentObservation),
    commandOwner: ({ context, event }) => event.type === 'COMMAND.CLOSE' && context.command === event.command,
    createdAttached: ({ context, event }) => event.type === 'CREATED' && event.pending === context.pending && context.pending?.destination.kind === 'creation' && authorityCurrent(context.client, context.pending) && event.control === context.control && event.generation === context.client.getSnapshot().generation && context.client.isAttachmentControlCurrent(event.sessionId, context.client.getSnapshot().views[event.sessionId]?.attachmentObservation),
    creating: ({ context, event }) => event.type === 'CREATE' && event.control === context.control && context.route.kind === 'new-conversation',
    changedAuthority: ({ context, event }) => event.type === 'AUTHORITY' && (context.authority !== event.authority || context.generation !== event.generation),
    resumingSession: ({ context, event }) => event.type === 'SESSION.RESUME' && context.route.kind === 'session-draft'
      && context.draftSession?.sessionId === context.route.sessionId && context.draftSession.intent === event.intent
      && (event.intent.destination.kind === 'session' || event.intent.destination.kind === 'restore-session')
      && !context.pending && event.intent.authority === event.authority
      && context.generation === event.generation && context.authority === event.authority
      && context.client.getSnapshot().connection === 'connected' && context.client.getSnapshot().generation === event.generation
      && context.client.getSnapshot().authorityRevision === event.authority
      && sessionRecoveryCurrent(context.client, context.route.sessionId),
  },
  actions: {
    navigationIntent: assign(({ context, event }) => {
      const intentRevision = context.intentRevision + 1, draft = context.draftSession;
      // Primary pages carry a currently eligible, previously admitted Home hint.
      // Replacement intents and unadmitted startup drafts keep stale revisions.
      if ((event.type === 'WORKFLOWS' || event.type === 'SCHEDULES')
        && draft?.wasAdmitted && draft.intent.destination.kind === 'restore-session' && draft.intentRevision === context.intentRevision
        && context.home.kind === 'session-draft' && context.home.sessionId === draft.sessionId) {
        return { intentRevision, draftSession: { ...draft, intentRevision } };
      }
      return { intentRevision };
    }),
    requestLeave: assign(({ context, event, self }) => {
      const intent = { event: event as NavigationEvent };
      context.workflows.send({ type: 'LEAVE', proceed: () => self.send({ type: 'WORKFLOW.LEFT', intent }), cancelled: () => self.send({ type: 'WORKFLOW.CANCELLED', intent }) });
      return { leaving: intent, approvedLeave: undefined };
    }),
    finishLeave: assign({ leaving: () => undefined, approvedLeave: () => undefined }),
    cancelWorkflowLeave: ({ context }) => { if (context.leaving) context.workflows.send({ type: 'CANCEL' }); },
    suspendSettings: ({ context }) => context.settings.send({ type: 'SUSPEND' }),
    page: assign(({ event }) => ({ route: { kind: event.type === 'SETTINGS' ? 'settings' as const : event.type === 'WORKFLOWS' ? 'workflows' as const : 'schedules' as const }, pending: undefined, command: undefined, retry: undefined, error: undefined, dismissedSidebar: undefined })),
    home: assign(({ context }) => ({ pending: { destination: { kind: 'home' as const, route: context.home, binding: context.binding }, route: context.route, generation: context.client.getSnapshot().generation, authority: context.client.getSnapshot().authorityRevision }, command: undefined, retry: undefined, error: undefined })),
    resumeSession: assign(({ context, event }) => {
      if (event.type !== 'SESSION.RESUME' || event.intent.destination.kind !== 'session' && event.intent.destination.kind !== 'restore-session') return {};
      // The logical editor outlives a failed or retired observation. Observe
      // already-admitted lifecycle work or a recovered same-claim proof,
      // without creating an Open or carrying a command/drawer interest.
      const destination = { ...event.intent.destination, sidebar: false, command: undefined };
      const pending: Pending = { recovery: true, destination, route: context.route, generation: event.generation, authority: event.authority };
      return { ...pendingSessionDraft(context, pending, destination.sessionId, context.draftSession!.intentRevision), pending, dismissedSidebar: undefined,
        retry: undefined, command: undefined, error: undefined };
    }),
    dismissSidebar: assign(({ context }) => ({ dismissedSidebar: context.pending })),
    start: assign(({ context, event }) => {
      if (event.type !== 'OPEN') return {};
      let destination = event.destination;
      if (destination.kind === 'session' && destination.nodeId && destination.conversationId) {
        const view = context.client.getSnapshot().views[destination.sessionId];
        if (context.client.isAttachmentControlCurrent(destination.sessionId, view?.attachmentObservation)
          && (view?.target?.conversation_id !== destination.conversationId || (view?.nodeId ?? view?.summary?.active_node) !== destination.nodeId)) {
          destination = { kind: 'node', sessionId: destination.sessionId, nodeId: destination.nodeId, conversationId: destination.conversationId };
        }
      }
      const pending: Pending = { destination, route: context.route, generation: context.client.getSnapshot().generation, authority: context.client.getSnapshot().authorityRevision };
      const retry = destination.kind === 'transition' && destination.action === 'retry' && destination.response
        ? responseRetry(context.client.getSnapshot().views[destination.sessionId], destination.response) : undefined;
      // Local selection never borrows another Session's editor or authority.
      // Its own executable promotion retains this same binding.
      return { ...((destination.kind === 'restore-session' || destination.kind === 'session') ? pendingSessionDraft(context, pending, destination.sessionId) : {}), pending, retry, dismissedSidebar: undefined, command: undefined, error: undefined };
    }),
    creationKnown: assign(({ context, event }) => event.type === 'CREATION.KNOWN' ? { route: { kind: 'session-draft' as const, sessionId: event.sessionId }, draftSession: { sessionId: event.sessionId, intent: event.pending, intentRevision: context.intentRevision, wasAdmitted: false } } : {}),
    create: assign(({ context, event }) => ({ pending: { destination: { kind: 'creation' as const, binding: event.type === 'CREATE' ? event.binding : String(context.binding) }, route: context.route, generation: context.client.getSnapshot().generation, authority: context.client.getSnapshot().authorityRevision }, command: undefined, error: undefined })),
    retire: assign({ dismissedSidebar: () => undefined, retry: () => undefined, draftSession: () => undefined, pending: () => undefined, command: () => undefined, error: () => undefined }),
    retirePreparation: assign(({ context }) => {
      const native = context.client.getSnapshot();
      // Losing a carrier retires this invocation, not the selected editor.
      // Its next connection may establish a new publication proof; explicit
      // cancellation, replacement navigation and authority change still retire it.
      const retained = selectedSessionDraft(context) && native.authorityRevision === context.draftSession!.intent.authority
        && (native.connection !== 'connected' || native.generation !== context.draftSession!.intent.generation);
      return { dismissedSidebar: undefined, retry: undefined, draftSession: retained ? context.draftSession : undefined,
        pending: undefined, command: undefined, error: undefined };
    }),
    commit: assign(({ context, event }) => {
      const output = (event as unknown as { output: Prepared }).output;
      if (output.pending.destination.kind === 'fork-session') return { dismissedSidebar: undefined, pending: undefined, error: output.warning, publication: { intent: output.pending, control: context.control, sidebar: false } };
      // Refocus/admission preserves the logical editor. Explicit executable
      // Node/Branch/Retry transitions still publish a fresh editor identity.
      const returning = output.pending.destination.kind === 'home';
      const creating = output.pending.destination.kind === 'creation';
      const preserveDraft = creating || (returning ? sameEditorDestination(context.home, output.route)
        || context.home.kind === 'new-conversation' && 'sessionId' in output.route && firstSubmissions(context.client).draft(String(context.binding))?.session?.id === output.route.sessionId
        : (output.pending.destination.kind === 'session' || output.pending.destination.kind === 'restore-session') && sameEditorDestination(context.route, output.route));
      // Returning or promoting the same logical editor cannot retire an
      // admitted submission. Native proof still gates executable controls.
      const control = preserveDraft ? context.control : {};
      // Home revalidation retains the original selection's intent and revision.
      // Recovery of a cold explicit selection must not turn it into a saved
      // startup selection before Native publishes executable controls.
      const restoringCommitted = returning && context.home.kind === 'session'
        && 'sessionId' in output.route && context.home.sessionId === output.route.sessionId;
      const draftSession = output.route.kind === 'session-draft'
        ? context.draftSession?.sessionId === output.route.sessionId ? context.draftSession
          : { sessionId: output.route.sessionId, intentRevision: context.intentRevision, wasAdmitted: restoringCommitted, intent: { recovery: true as const, destination: { kind: restoringCommitted ? 'restore-session' as const : 'session' as const, sessionId: output.route.sessionId, sidebar: false }, route: output.route, generation: output.pending.generation, authority: output.pending.authority } }
        : undefined;
      return { dismissedSidebar: undefined, retry: context.retry && output.replay ? { ...context.retry, replay: output.replay } : undefined, draftSession, binding: preserveDraft ? context.binding : context.binding + 1, control, route: output.route, pending: undefined, error: undefined,
        command: (output.pending.destination.kind === 'session' || output.pending.destination.kind === 'restore-session') && output.pending.destination.command ? { request: output.pending.destination.command, route: output.route, generation: output.pending.generation } : undefined,
        restored: output.route.kind === 'session' && output.content ? { conversation: output.observation!.target.conversation_id, content: output.content } : preserveDraft && (returning || context.restored?.conversation === output.observation?.target.conversation_id) ? context.restored : undefined,
        publication: { intent: output.pending, control, sidebar: (output.pending.destination.kind === 'session' || output.pending.destination.kind === 'restore-session') && !!output.pending.destination.sidebar && context.dismissedSidebar !== output.pending } };
    }),
    attached: assign(({ context, event }) => {
      if (event.type !== 'OPEN' || event.destination.kind !== 'session' && event.destination.kind !== 'restore-session') return {};
      const route: ShellRoute = context.route.kind === 'session' && context.route.sessionId === event.destination.sessionId ? context.route : { kind: 'session', sessionId: event.destination.sessionId };
      const preserveDraft = sameEditorDestination(context.route, route);
      const control = preserveDraft ? context.control : {};
      return { dismissedSidebar: undefined, retry: undefined, draftSession: undefined, binding: preserveDraft ? context.binding : context.binding + 1, control, route, pending: undefined, error: undefined, restored: preserveDraft && context.restored?.conversation === context.client.getSnapshot().views[event.destination.sessionId]?.target?.conversation_id ? context.restored : undefined, publication: { control, sidebar: event.destination.sidebar }, command: event.destination.command ? { request: event.destination.command, route, generation: context.client.getSnapshot().generation } : undefined };
    }),
    fail: assign(({ event }) => ({ dismissedSidebar: undefined, retry: undefined, pending: undefined, error: actionError((event as unknown as { error: unknown }).error) })),
    draft: assign(({ context, event }) => {
      if (event.type !== 'DRAFT' && event.type !== 'DRAFT.WORKSPACE') return {};
      // Registered Workspace metadata refines a resident draft. A completed or
      // native-owned submission is a different draft when New Conversation is chosen.
      const flow = firstSubmissions(context.client).draft(String(context.binding));
      const submitted = !!(flow?.session || flow?.existingSessionId);
      const resident = context.route.kind === 'new-conversation' && !context.pending && (!flow || !submitted && ['rejected', 'discarded'].includes(flow.phase));
      const preserve = !submitted && (event.type === 'DRAFT.WORKSPACE' || resident && context.route.kind === 'new-conversation' && (event.workspaceId !== undefined || context.route.workspaceId === event.workspaceId));
      const route = preserve && event.type === 'DRAFT' && context.route.kind === 'new-conversation' && context.route.workspaceId === event.workspaceId ? context.route : { kind: 'new-conversation' as const, workspaceId: event.workspaceId };
      const draftWorkspaceSession = event.type === 'DRAFT' && event.workspaceId === undefined
        ? 'sessionId' in context.home ? context.home.sessionId : context.draftWorkspaceSession
        : undefined;
      return { retry: undefined, draftSession: undefined, draftWorkspaceSession, binding: preserve ? context.binding : context.binding + 1, control: preserve ? context.control : {}, route, pending: undefined, command: undefined, restored: undefined, error: undefined };
    }),
    created: assign(({ context, event }) => event.type === 'CREATED' ? { retry: undefined, draftSession: undefined, binding: context.binding, control: context.control, route: { kind: 'session' as const, sessionId: event.sessionId }, pending: undefined, command: undefined, error: undefined } : {}),
    command: assign(({ context, event }) => ({ retry: undefined, pending: undefined, command: event.type === 'COMMAND' && event.request ? { request: event.request, route: context.route, generation: context.client.getSnapshot().generation } : undefined })),
    retireSettings: ({ context, event }) => { if (event.type === 'AUTHORITY' && context.authority !== event.authority) context.settings.send({ type: 'RETIRE' }); },
    settings: ({ context, event }) => { if (event.type === 'SETTINGS') context.settings.send(event.destination ?? { type: 'SUSPEND' }); },
    authority: assign(({ context, event }) => event.type === 'AUTHORITY' ? { dismissedSidebar: undefined, retry: undefined, draftWorkspaceSession: context.authority !== event.authority ? undefined : context.draftWorkspaceSession, draftSession: context.authority !== event.authority ? undefined : context.draftSession, binding: context.authority !== event.authority ? context.binding + 1 : context.binding, control: {}, generation: event.generation, authority: event.authority, pending: undefined, command: undefined, error: undefined,
      ...(context.authority !== event.authority ? { home: { kind: 'new-conversation' as const }, route: { kind: 'new-conversation' as const }, leaving: undefined, approvedLeave: undefined } : {}),
    } : {}),
    dismissError: assign({ error: () => undefined }),
  },
}).createMachine({
  id: 'shellNavigation', initial: 'idle',
  invoke: [{ id: 'nativeAuthority', src: 'authority', input: ({ context, self }) => ({ client: context.client, parent: self }) }, { id: 'draftOwnership', src: 'drafts', input: ({ context, self }) => ({ owner: context.drafts, parent: self }) }],
  context: ({ input }) => ({ ...input, binding: 0, intentRevision: 0, control: {}, home: input.route && isHomeRoute(input.route) ? input.route : { kind: 'new-conversation' }, route: input.route ?? { kind: 'new-conversation' }, generation: input.client.getSnapshot().generation, authority: input.client.getSnapshot().authorityRevision }),
  // Adoption can precede the replay acknowledgement. Retire presentation on
  // either ordering without depending on a mounted Chat surface.
  always: [
    { guard: ({ context }) => isHomeRoute(context.route) && context.home !== context.route, actions: assign(({ context }) => ({ home: context.route as ConversationRoute })) },
    { guard: ({ context }) => !!context.retry && responseRetryReady(context.retry, context.client.getSnapshot().views[context.retry.view.id]), actions: assign({ retry: () => undefined }) },
  ],
  states: {
    idle: { always: { guard: 'resumingCreation', target: 'preparing', actions: 'create' } },
    preparing: { invoke: { id: 'preparation', src: 'prepare', input: ({ context, self }) => ({ client: context.client, host: context.host, workspaces: context.workspaces, pending: context.pending!, parent: self }),
      onDone: [{ guard: 'prepared', target: 'idle', actions: 'commit' }, { target: 'idle', actions: 'retirePreparation' }],
      onError: [{ guard: 'failed', target: 'idle', actions: 'fail' }, { target: 'idle', actions: 'retirePreparation' }] } },
  },
  on: {
    'RETRY.OBSERVED': { guard: ({ context, event }) => context.retry === event.retry && responseRetryReady(event.retry, context.client.getSnapshot().views[event.retry.view.id]), actions: assign({ retry: () => undefined }) },
    'CREATION.KNOWN': { guard: 'createdOwner', actions: 'creationKnown' },
    'WORKFLOW.LEFT': { guard: ({ context, event }) => context.leaving === event.intent && context.route.kind === 'workflows', actions: [assign(({ event }) => ({ approvedLeave: event.intent })), ({ event, self }) => self.send(event.intent.event)] },
    'WORKFLOW.CANCELLED': { guard: ({ context, event }) => context.leaving === event.intent, actions: 'finishLeave' },
    OPEN: [{ guard: 'workflowLeave', actions: 'requestLeave' }, { guard: 'sameTransition' }, { guard: 'attached', target: '.idle', reenter: true, actions: ['navigationIntent', 'suspendSettings', 'finishLeave', 'attached'] }, { target: '.preparing', reenter: true, actions: ['navigationIntent', 'suspendSettings', 'finishLeave', 'start'] }],
    DRAFT: [{ guard: 'workflowLeave', actions: 'requestLeave' }, { guard: 'draftTargetAvailable', target: '.idle', actions: ['navigationIntent', 'suspendSettings', 'finishLeave', 'draft'] }], 'DRAFT.WORKSPACE': { guard: 'drafting', target: '.idle', actions: 'draft' },
    CREATE: { guard: 'creating', target: '.preparing', reenter: true, actions: ['navigationIntent', 'create'] },
    CREATED: { guard: 'createdAttached', target: '.idle', actions: 'created' },
    COMMAND: { guard: 'commandRoute', target: '.idle', actions: 'command' },
    'COMMAND.CLOSE': { guard: 'commandOwner', actions: assign({ command: () => undefined }) },
    // Primary routes retire disposable publication only. The Home editor,
    // admitted native operations and configuration transactions keep their owners.
    HOME: [{ guard: 'workflowLeave', actions: 'requestLeave' }, { guard: ({ context }) => isHomeRoute(context.route) }, { target: '.preparing', reenter: true, actions: ['suspendSettings', 'finishLeave', 'home'] }],
    SETTINGS: [{ guard: 'workflowLeave', actions: 'requestLeave' }, { target: '.idle', actions: ['finishLeave', 'page', 'settings'] }],
    WORKFLOWS: [{ guard: ({ context }) => context.route.kind === 'workflows', target: '.idle', actions: ['navigationIntent', 'cancelWorkflowLeave', 'finishLeave', 'page'] }, { target: '.idle', actions: ['navigationIntent', 'suspendSettings', 'page'] }],
    SCHEDULES: [{ guard: 'workflowLeave', actions: 'requestLeave' }, { target: '.idle', actions: ['navigationIntent', 'suspendSettings', 'finishLeave', 'page'] }],
    'SIDEBAR.DISMISS': [{ guard: 'selectedSessionDraft', actions: 'dismissSidebar' }, { guard: 'sidebar', target: '.idle', actions: 'retire' }],
    CANCEL: { guard: 'cancelling', target: '.idle', actions: 'retire' },
    AUTHORITY: { guard: 'changedAuthority', target: '.idle', actions: ['retireSettings', 'authority'] },
    'SESSION.RESUME': { guard: 'resumingSession', target: '.preparing', reenter: true, actions: 'resumeSession' },
    'DISMISS.ERROR': { actions: 'dismissError' },
  },
});
export type ShellNavigationActor = ActorRefFrom<typeof shellNavigationMachine>;
/** Committed controls belong to the retained Home owner and native connection,
 * independently of the disposable preparing actor and visible primary page. */
export function committedRouteCurrent(actor: ShellNavigationActor) {
  const { control, client } = actor.getSnapshot().context, generation = client.getSnapshot().generation, authority = client.getSnapshot().authorityRevision;
  return () => actor.getSnapshot().status === 'active' && actor.getSnapshot().context.control === control && client.getSnapshot().generation === generation && client.getSnapshot().authorityRevision === authority;
}

/** Local startup input belongs to its retained Home editor, including primary
 * page round trips. Native attachment/admission still gates executable I/O. */
export function draftSessionCurrent(actor: ShellNavigationActor, intent: Pending) {
  const { client, binding, draftSession } = actor.getSnapshot().context;
  const sessionId = draftSession?.intent === intent ? draftSession.sessionId : undefined;
  return () => {
    const context = actor.getSnapshot().context;
    const retained = sessionId !== undefined && context.binding === binding
      && 'sessionId' in context.home && context.home.sessionId === sessionId;
    return actor.getSnapshot().status === 'active' && authorityCurrent(client, intent)
      && (retained || context.pending === intent || context.publication?.intent === intent && context.publication.control === context.control);
  };
}
