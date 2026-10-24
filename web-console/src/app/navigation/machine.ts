import { assign, fromCallback, fromPromise, setup, type ActorRefFrom } from 'xstate';
import type { AppServerClient } from '../../client/app-server';
import type { OperationAdmission } from '../../client/contracts';
import type { ConversationWindow, CompletedResponseView, UserInputBlock } from '../../../../protocol/app-server/v57';
import type { Observation } from '../../client/session-lifecycle/port';
import { CommandSession, type HistoricalSelection, type HistoryAction } from '../commands/native';
import type { CommandRequest } from '../commands/CommandPanel';
import type { ForkOrigin } from '../agent/ForkPoint';
import type { SettingsNavigationActor, SettingsNavigationEvent } from '../settings/machines/navigation';
import { installTranscriptWindow } from '../../client/transcript';
import { firstSubmissions } from '../new-conversation/first-submit';
import { responseRetry, responseRetryReady, type ResponseRetry } from '../agent/response-retry';
import { forkLatestSession } from '../commands/fork-session';

class RetiredPreparation extends Error {}
export type ConversationRoute = { kind: 'new-conversation'; workspaceId?: string } | { kind: 'session'; sessionId: string } | { kind: 'session-draft'; sessionId: string };
export interface HistoryRoute {
  kind: 'history'; sessionId: string; nodeId: string; messageId?: string;
  conversationId: string; window: ConversationWindow; returnTo: ConversationRoute;
}
/** The Workflows content area (Issue #491). It owns no Session or
 * conversation: its file targets live in the Workflows surface actor. */
export interface WorkflowsRoute { kind: 'workflows' }
export type ShellRoute = ConversationRoute | HistoryRoute | WorkflowsRoute;
/** History changes the visible surface, not its originating editor destination.
 * Native Conversation/receipt authority is reconciled separately by Composer. */
function editorDestination(route: ShellRoute): Exclude<ShellRoute, HistoryRoute> {
  return route.kind === 'history' ? route.returnTo : route;
}
function sameEditorDestination(a: ShellRoute, b: ShellRoute) {
  const from = editorDestination(a), to = editorDestination(b);
  return 'sessionId' in from && 'sessionId' in to ? from.sessionId === to.sessionId
    : from.kind === 'new-conversation' && to.kind === 'new-conversation' && from.workspaceId === to.workspaceId;
}
type Destination =
  | { kind: 'creation'; binding: string }
  | { kind: 'session'; sessionId: string; sidebar: boolean; command?: CommandRequest }
  | { kind: 'restore-session'; sessionId: string; sidebar: boolean; command?: CommandRequest }
  | { kind: 'history'; origin: ForkOrigin; beginViewport?: (retired: () => void) => { current: () => boolean } }
  | { kind: 'transition'; sessionId: string; action: HistoryAction; response?: CompletedResponseView; selection?: HistoricalSelection }
  | { kind: 'fork-session'; sessionId: string; sidebar?: boolean }
  | { kind: 'node'; sessionId: string; nodeId: string; conversationId: string };
interface Pending { viewportCurrent?: () => boolean; observation?: Observation; destination: Destination; route: ShellRoute; generation: number; authority: number | undefined }
interface Prepared { warning?: string; pending: Pending; route: ShellRoute; observation?: Observation; content?: UserInputBlock[]; replay?: ResponseRetry['replay'] }
interface Input { client: AppServerClient; settings: SettingsNavigationActor; route?: ConversationRoute }
export interface ShellNavigationContext extends Omit<Input, 'route'> {
  // Draft identity survives temporary history; control identity never does.
  binding: number; control: object; route: ShellRoute; pending?: Pending; error?: string;
  retry?: ResponseRetry;
  command?: { request: CommandRequest; route: ShellRoute; generation: number };
  readAdmission?: OperationAdmission;
  restored?: { conversation: string; content: UserInputBlock[] };
  draftSession?: { sessionId: string; intent: Pending };
  publication?: { sidebar: boolean; intent?: Pending; control: object };
  generation: number; authority: number | undefined;
}
export type ShellNavigationEvent =
  | { type: 'OPEN'; destination: Destination }
  | { type: 'DRAFT'; workspaceId?: string }
  | { type: 'DRAFT.WORKSPACE'; workspaceId: string }
  | { type: 'CREATION.KNOWN'; pending: Pending; sessionId: string }
  | { type: 'HISTORY.ADMITTED'; pending: Pending; admission: OperationAdmission }
  | { type: 'RETRY.OBSERVED'; retry: ResponseRetry }
  | { type: 'CREATE'; binding: string; control: object }
  | { type: 'CREATED'; sessionId: string; control: object; generation: number; pending?: Pending }
  | { type: 'RETURN'; route: HistoryRoute }
  | { type: 'COMMAND'; request: CommandRequest }
  | { type: 'COMMAND.CLOSE'; command: NonNullable<ShellNavigationContext['command']> }
  | { type: 'SETTINGS'; destination: Extract<SettingsNavigationEvent, { type: 'OPEN' | 'OPEN.CONNECTION' | 'OPEN.OWNER' | 'OPEN.REPAIRS' }> }
  | { type: 'WORKFLOWS' }
  | { type: 'SIDEBAR.DISMISS' | 'DISMISS.ERROR' }
  | { type: 'CANCEL'; pending?: Pending }
  | { type: 'AUTHORITY'; generation: number; authority: number | undefined };

function authorityCurrent(client: AppServerClient, pending: Pending) {
  const state = client.getSnapshot();
  return state.connection === 'connected' && state.generation === pending.generation && state.authorityRevision === pending.authority && (!pending.viewportCurrent || pending.viewportCurrent())
    && (pending.destination.kind !== 'history' || pending.route.kind !== 'session' || client.isAttachmentObservationCurrent(pending.route.sessionId, pending.observation));
}
/** Disposable preparation owns publication only. Native calls below are finite
 * lifecycle commands: aborting this actor retires admission, never settlement. */
async function prepare(input: { client: AppServerClient; pending: Pending; parent: { send(event: ShellNavigationEvent): void } }, signal: AbortSignal): Promise<Prepared> {
  const { client, pending } = input, destination = pending.destination;
  const current = () => !signal.aborted && authorityCurrent(client, pending);
  if (destination.kind === 'creation') {
    // Observe the native FirstSubmissions owner; this actor never creates,
    // attaches or executes the draft. Publication arrives as correlated CREATED.
    return await new Promise<never>((_resolve, reject) => {
      const inspect = () => {
        const flow = firstSubmissions(client).draft(destination.binding);
        if (flow?.session) input.parent.send({ type: 'CREATION.KNOWN', pending, sessionId: flow.session.id });
        if (flow && ['failed', 'rejected', 'uncertain', 'discarded', 'paused'].includes(flow.phase)) { cleanup(); reject(flow.error ?? new Error('Conversation preparation stopped.')); }
      };
      const release = firstSubmissions(client).subscribe(inspect);
      const retired = () => { cleanup(); reject(new Error('Conversation publication retired.')); };
      const cleanup = () => { release(); signal.removeEventListener('abort', retired); };
      signal.addEventListener('abort', retired, { once: true }); inspect();
    });
  }
  if (destination.kind === 'history') {
    const origin = destination.origin;
    const allowed = await client.admitSessionOperation(origin.source_session, current);
    if (!allowed || !current()) throw new RetiredPreparation();
    input.parent.send({ type: 'HISTORY.ADMITTED', pending, admission: allowed });
    const { node } = await client.request({ method: 'session/node', params: { session_id: origin.source_session, node_id: origin.source_node } }, 'node', undefined, allowed);
    if (!current() || !allowed.current()) throw new RetiredPreparation();
    if (node.id !== origin.source_node) throw new Error('Different source Node returned.');
    const ref=origin.source_message_ref;
    const {conversation_id,window}=origin.source_message!=null
      ? await client.request({method:'session/history',params:{session_id:origin.source_session,node_id:origin.source_node,at:{type:'message',id:origin.source_message},limit:32}},'session_history',undefined,allowed)
      : ref ? await client.request({method:'session/nodeOriginHistory',params:{session_id:ref.session_id,node_id:ref.node_id,limit:32}},'session_history',undefined,allowed)
      : (()=>{throw new Error('The native source Message locator is unavailable.');})();
    installTranscriptWindow(window);
    if (window.page.entries?.some(entry => BigInt(entry.cursor) > BigInt(window.cut.transcript))) throw new Error('History entries exceed the native read cut.');
    const exact = window.page.entries?.find(entry => entry.cursor === window.target_cursor);
    if (conversation_id !== node.conversation_id || window.cut.conversation_id !== conversation_id || (origin.source_message!=null?(exact?.item.type !== 'message'||exact.item.message.id!==origin.source_message):window.target_cursor==null)) throw new Error('History read did not resolve the exact source Message.');
    const returnTo = pending.route.kind === 'history' ? pending.route.returnTo : pending.route;
    // A Fork source opens from a conversation, never from the Workflows area.
    if (returnTo.kind === 'workflows') throw new RetiredPreparation();
    return { pending, route: { kind: 'history', sessionId: origin.source_session, nodeId: node.id, messageId: origin.source_message??undefined, conversationId: conversation_id, window, returnTo } };
  }
  let sessionId = destination.sessionId, content: UserInputBlock[] | undefined, replay: ResponseRetry['replay'];
  let readyObservation: Observation | undefined;
  if (destination.kind === 'session' || destination.kind === 'restore-session') {
    if (destination.kind === 'restore-session' && firstSubmissions(client).session(sessionId) && !['admitted', 'discarded'].includes(firstSubmissions(client).session(sessionId)!.phase)) {
      if (firstSubmissions(client).session(sessionId)!.phase === 'attaching') await client.waitForAttachment(sessionId, current);
      if (!client.isAttachmentControlCurrent(sessionId, client.getSnapshot().views[sessionId]?.attachmentObservation)) return { pending, route: { kind: 'session-draft', sessionId } };
    } else if (!client.isAttachmentControlCurrent(sessionId, client.getSnapshot().views[sessionId]?.attachmentObservation)) {
      // Native attachment evidence admits publication; supplementary metadata
      // settlement continues in Session Lifecycle after this disposable read.
      await new Promise<void>((resolve, reject) => {
        void client.attach(sessionId, undefined, current, () => {
          const proof = client.getSnapshot().views[sessionId]?.attachmentObservation;
          if (client.isAttachmentControlCurrent(sessionId, proof)) readyObservation = proof;
          resolve();
        }).then(resolve, reject);
      });
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
 * XState's correlated invoke completion plus the final proof guard is the route
 * linearization point. Failure and supersession preserve the committed executable
 * route; selecting local startup input is an explicit draft replacement. */
export const shellNavigationMachine = setup({
  types: { context: {} as ShellNavigationContext, input: {} as Input, events: {} as ShellNavigationEvent },
  actors: { authority: fromCallback<ShellNavigationEvent, { client: AppServerClient; parent: { getSnapshot(): { context: Pick<ShellNavigationContext, 'generation' | 'authority' | 'retry'> } } }>(({ input, sendBack }) => {
    const observe = () => {
      const native = input.client.getSnapshot(), owner = input.parent.getSnapshot().context;
      if (native.generation !== owner.generation || native.authorityRevision !== owner.authority) sendBack({ type: 'AUTHORITY', generation: native.generation, authority: native.authorityRevision });
      else if (owner.retry && responseRetryReady(owner.retry, native.views[owner.retry.view.id])) sendBack({ type: 'RETRY.OBSERVED', retry: owner.retry });
    };
    const release = input.client.subscribe(observe); observe(); return release;
  }), prepare: fromPromise<Prepared, { client: AppServerClient; pending: Pending; parent: { send(event: ShellNavigationEvent): void } }>(({ input, signal }) => prepare(input, signal)) },
  guards: {
    prepared: ({ context, event }) => {
      const output = (event as unknown as { output: Prepared }).output;
      return output.pending === context.pending && authorityCurrent(context.client, output.pending)
        && (!context.readAdmission || context.readAdmission.current())
        && (output.route.kind !== 'session' || context.client.isAttachmentObservationCurrent(output.route.sessionId, output.observation));
    },
    createdOwner: ({ context, event }) => event.type === 'CREATION.KNOWN' && context.pending === event.pending && context.pending.destination.kind === 'creation',
    readOwner: ({ context, event }) => event.type === 'HISTORY.ADMITTED' && event.pending === context.pending,
    failed: ({ context, event }) => !!context.pending && authorityCurrent(context.client, context.pending) && (!context.readAdmission || context.readAdmission.current()) && !((event as unknown as { error: unknown }).error instanceof RetiredPreparation),
    attached: ({ context, event }) => event.type === 'OPEN' && (event.destination.kind === 'session' || event.destination.kind === 'restore-session')
      && context.client.isAttachmentControlCurrent(event.destination.sessionId, context.client.getSnapshot().views[event.destination.sessionId]?.attachmentObservation),
    cancelling: ({ context, event }) => event.type === 'CANCEL' && (!event.pending || context.pending === event.pending),
    sidebar: ({ context }) => (context.pending?.destination.kind === 'session' || context.pending?.destination.kind === 'fork-session') && !!context.pending.destination.sidebar,
    drafting: ({ context }) => context.route.kind === 'new-conversation',
    commandRoute: ({ context }) => context.route.kind === 'session' && context.client.isAttachmentControlCurrent(context.route.sessionId, context.client.getSnapshot().views[context.route.sessionId]?.attachmentObservation),
    commandOwner: ({ context, event }) => event.type === 'COMMAND.CLOSE' && context.command === event.command,
    returning: ({ context, event }) => event.type === 'RETURN' && context.route === event.route,
    returnCold: ({ context, event }) => event.type === 'RETURN' && context.route === event.route && (event.route.returnTo.kind === 'session' || event.route.returnTo.kind === 'session-draft') && !context.client.isAttachmentControlCurrent(event.route.returnTo.sessionId, context.client.getSnapshot().views[event.route.returnTo.sessionId]?.attachmentObservation),
    createdAttached: ({ context, event }) => event.type === 'CREATED' && event.pending === context.pending && context.pending?.destination.kind === 'creation' && authorityCurrent(context.client, context.pending) && event.control === context.control && event.generation === context.client.getSnapshot().generation && context.client.isAttachmentControlCurrent(event.sessionId, context.client.getSnapshot().views[event.sessionId]?.attachmentObservation),
    creating: ({ context, event }) => event.type === 'CREATE' && event.control === context.control && context.route.kind === 'new-conversation',
    changedAuthority: ({ context, event }) => event.type === 'AUTHORITY' && (context.authority !== event.authority || context.generation !== event.generation),
  },
  actions: {
    readAdmitted: assign(({ event }) => event.type === 'HISTORY.ADMITTED' ? { readAdmission: event.admission } : {}),
    start: assign(({ context, event, self }) => {
      if (event.type !== 'OPEN') return {};
      const pending: Pending = { observation: context.route.kind === 'session' ? context.client.getSnapshot().views[context.route.sessionId]?.attachmentObservation : undefined, destination: event.destination, route: context.route, generation: context.client.getSnapshot().generation, authority: context.client.getSnapshot().authorityRevision };
      const destination = event.destination;
      const retry = destination.kind === 'transition' && destination.action === 'retry' && destination.response
        ? responseRetry(context.client.getSnapshot().views[destination.sessionId], destination.response) : undefined;
      if (event.destination.kind === 'history' && event.destination.beginViewport) {
        // Registration and intent construction are one synchronous gesture.
        // The ticket also samples native scroll movement at final publication.
        pending.viewportCurrent = event.destination.beginViewport(() => self.send({ type: 'CANCEL', pending })).current;
      }
      const startup = (context.route.kind === 'new-conversation' || context.route.kind === 'session-draft') && (event.destination.kind === 'restore-session' || event.destination.kind === 'session');
      // Local draft replacement retires its own input/control owner. Only that
      // draft's subsequent executable promotion may retain its binding.
      const replacingDraft = startup && 'sessionId' in event.destination && (context.route.kind !== 'session-draft' || context.route.sessionId !== event.destination.sessionId);
      return { pending, retry, binding: replacingDraft ? context.binding + 1 : context.binding, control: replacingDraft ? {} : context.control, route: startup && 'sessionId' in event.destination ? { kind: 'session-draft' as const, sessionId: event.destination.sessionId } : context.route, draftSession: startup && 'sessionId' in event.destination ? { sessionId: event.destination.sessionId, intent: pending } : context.draftSession, readAdmission: undefined, command: undefined, error: undefined };
    }),
    creationKnown: assign(({ event }) => event.type === 'CREATION.KNOWN' ? { route: { kind: 'session-draft' as const, sessionId: event.sessionId }, draftSession: { sessionId: event.sessionId, intent: event.pending } } : {}),
    create: assign(({ context, event }) => event.type === 'CREATE' ? { pending: { destination: { kind: 'creation' as const, binding: event.binding }, route: context.route, generation: context.client.getSnapshot().generation, authority: context.client.getSnapshot().authorityRevision }, readAdmission: undefined, command: undefined, error: undefined } : {}),
    retire: assign({ retry: () => undefined, draftSession: () => undefined, readAdmission: () => undefined, pending: () => undefined, command: () => undefined, error: () => undefined }),
    commit: assign(({ context, event }) => {
      const output = (event as unknown as { output: Prepared }).output;
      if (output.pending.destination.kind === 'fork-session') return { pending: undefined, error: output.warning, publication: { intent: output.pending, control: context.control, sidebar: false } };
      const control = {};
      // Refocus/admission preserves the logical editor. Explicit executable
      // Node/Branch/Retry transitions still publish a fresh editor identity.
      const preserveDraft = (output.pending.destination.kind === 'session' || output.pending.destination.kind === 'restore-session') && sameEditorDestination(context.route, output.route);
      return { retry: context.retry && output.replay ? { ...context.retry, replay: output.replay } : undefined, draftSession: undefined, readAdmission: undefined, binding: preserveDraft || output.route.kind === 'history' ? context.binding : context.binding + 1, control, route: output.route, pending: undefined, error: undefined,
        command: (output.pending.destination.kind === 'session' || output.pending.destination.kind === 'restore-session') && output.pending.destination.command ? { request: output.pending.destination.command, route: output.route, generation: output.pending.generation } : undefined,
        restored: output.route.kind === 'history' ? context.restored : output.route.kind === 'session' && output.content ? { conversation: output.observation!.target.conversation_id, content: output.content } : preserveDraft && context.restored?.conversation === output.observation?.target.conversation_id ? context.restored : undefined,
        publication: { intent: output.pending, control, sidebar: (output.pending.destination.kind === 'session' || output.pending.destination.kind === 'restore-session') && !!output.pending.destination.sidebar } };
    }),
    attached: assign(({ context, event }) => {
      if (event.type !== 'OPEN' || event.destination.kind !== 'session' && event.destination.kind !== 'restore-session') return {};
      const route: ShellRoute = context.route.kind === 'session' && context.route.sessionId === event.destination.sessionId ? context.route : { kind: 'session', sessionId: event.destination.sessionId };
      const preserveDraft = sameEditorDestination(context.route, route);
      const control = route === context.route ? context.control : {};
      return { retry: undefined, draftSession: undefined, readAdmission: undefined, binding: preserveDraft ? context.binding : context.binding + 1, control, route, pending: undefined, error: undefined, restored: preserveDraft && context.restored?.conversation === context.client.getSnapshot().views[event.destination.sessionId]?.target?.conversation_id ? context.restored : undefined, publication: { control, sidebar: event.destination.sidebar }, command: event.destination.command ? { request: event.destination.command, route, generation: context.client.getSnapshot().generation } : undefined };
    }),
    fail: assign(({ event }) => ({ retry: undefined, readAdmission: undefined, pending: undefined, error: String((event as unknown as { error: unknown }).error) })),
    draft: assign(({ context, event }) => {
      if (event.type !== 'DRAFT' && event.type !== 'DRAFT.WORKSPACE') return {};
      // Registered Workspace metadata refines a resident draft. A completed or
      // native-owned submission is a different draft when New Conversation is chosen.
      const flow = firstSubmissions(context.client).draft(String(context.binding));
      const resident = context.route.kind === 'new-conversation' && !context.pending && (!flow || ['rejected', 'discarded'].includes(flow.phase));
      const preserve = event.type === 'DRAFT.WORKSPACE' || resident && context.route.kind === 'new-conversation' && context.route.workspaceId === event.workspaceId;
      const route = preserve && event.type === 'DRAFT' ? context.route : { kind: 'new-conversation' as const, workspaceId: event.workspaceId };
      return { retry: undefined, draftSession: undefined, readAdmission: undefined, binding: preserve ? context.binding : context.binding + 1, control: preserve ? context.control : {}, route, pending: undefined, command: undefined, restored: undefined, error: undefined };
    }),
    created: assign(({ context, event }) => event.type === 'CREATED' ? { retry: undefined, draftSession: undefined, readAdmission: undefined, binding: context.binding, control: {}, route: { kind: 'session' as const, sessionId: event.sessionId }, pending: undefined, command: undefined, error: undefined } : {}),
    returnPrepare: assign(({ context, event }) => event.type === 'RETURN' && (event.route.returnTo.kind === 'session' || event.route.returnTo.kind === 'session-draft') ? { pending: { destination: { kind: 'session' as const, sessionId: event.route.returnTo.sessionId, sidebar: false }, route: context.route, generation: context.client.getSnapshot().generation, authority: context.client.getSnapshot().authorityRevision }, readAdmission: undefined, command: undefined, error: undefined } : {}),
    back: assign(({ context }) => ({ retry: undefined, draftSession: undefined, readAdmission: undefined, binding: context.binding, control: {}, route: editorDestination(context.route), pending: undefined, error: undefined })),
    command: assign(({ context, event }) => ({ retry: undefined, readAdmission: undefined, pending: undefined, command: event.type === 'COMMAND' && event.request ? { request: event.request, route: context.route, generation: context.client.getSnapshot().generation } : undefined })),
    retireSettings: ({ context, event }) => { if (event.type === 'AUTHORITY' && context.authority !== event.authority) context.settings.send({ type: 'RETIRE' }); },
    settings: ({ context, event }) => { if (event.type === 'SETTINGS') context.settings.send(event.destination); },
    authority: assign(({ context, event }) => event.type === 'AUTHORITY' ? { retry: undefined, draftSession: context.authority !== event.authority ? undefined : context.draftSession, readAdmission: undefined, binding: context.authority !== event.authority ? context.binding + 1 : context.binding, control: {}, generation: event.generation, authority: event.authority, pending: undefined, command: undefined, error: undefined,
      route: context.authority !== event.authority ? { kind: 'new-conversation' as const } : context.route.kind === 'history' ? context.route.returnTo : context.route } : {}),
    dismissError: assign({ error: () => undefined }),
  },
}).createMachine({
  id: 'shellNavigation', initial: 'idle',
  invoke: { id: 'nativeAuthority', src: 'authority', input: ({ context, self }) => ({ client: context.client, parent: self }) },
  context: ({ input }) => ({ ...input, binding: 0, control: {}, route: input.route ?? { kind: 'new-conversation' }, generation: input.client.getSnapshot().generation, authority: input.client.getSnapshot().authorityRevision }),
  // Adoption can precede the replay acknowledgement. Retire presentation on
  // either ordering without depending on a mounted Chat surface.
  always: { guard: ({ context }) => !!context.retry && responseRetryReady(context.retry, context.client.getSnapshot().views[context.retry.view.id]), actions: assign({ retry: () => undefined }) },
  states: {
    idle: {},
    preparing: { invoke: { id: 'preparation', src: 'prepare', input: ({ context, self }) => ({ client: context.client, pending: context.pending!, parent: self }),
      onDone: [{ guard: 'prepared', target: 'idle', actions: 'commit' }, { target: 'idle', actions: 'retire' }],
      onError: [{ guard: 'failed', target: 'idle', actions: 'fail' }, { target: 'idle', actions: 'retire' }] } },
  },
  on: {
    'RETRY.OBSERVED': { guard: ({ context, event }) => context.retry === event.retry && responseRetryReady(event.retry, context.client.getSnapshot().views[event.retry.view.id]), actions: assign({ retry: () => undefined }) },
    'CREATION.KNOWN': { guard: 'createdOwner', actions: 'creationKnown' },
    'HISTORY.ADMITTED': { guard: 'readOwner', actions: 'readAdmitted' },
    OPEN: [{ guard: 'attached', target: '.idle', reenter: true, actions: 'attached' }, { target: '.preparing', reenter: true, actions: 'start' }],
    DRAFT: { target: '.idle', actions: 'draft' }, 'DRAFT.WORKSPACE': { guard: 'drafting', target: '.idle', actions: 'draft' },
    CREATE: { guard: 'creating', target: '.preparing', reenter: true, actions: 'create' },
    CREATED: { guard: 'createdAttached', target: '.idle', actions: 'created' }, RETURN: [{ guard: 'returnCold', target: '.preparing', reenter: true, actions: 'returnPrepare' }, { guard: 'returning', target: '.idle', actions: 'back' }],
    COMMAND: { guard: 'commandRoute', target: '.idle', actions: 'command' },
    'COMMAND.CLOSE': { guard: 'commandOwner', actions: assign({ command: () => undefined }) },
    SETTINGS: { target: '.idle', actions: ['retire', 'settings'] },
    // Leaving a conversation for Workflows retires its pending intent and
    // its committed controls; it cancels no native work.
    WORKFLOWS: { target: '.idle', actions: ['retire', assign({ route: () => ({ kind: 'workflows' as const }), control: () => ({}) })] },
    'SIDEBAR.DISMISS': { guard: 'sidebar', target: '.idle', actions: 'retire' },
    CANCEL: { guard: 'cancelling', target: '.idle', actions: 'retire' },
    AUTHORITY: { guard: 'changedAuthority', target: '.idle', actions: ['retireSettings', 'authority'] },
    'DISMISS.ERROR': { actions: 'dismissError' },
  },
});
export type ShellNavigationActor = ActorRefFrom<typeof shellNavigationMachine>;
/** Committed controls are keyed to a route object and native connection, never
 * to the disposable preparing actor. */
export function committedRouteCurrent(actor: ShellNavigationActor) {
  const { control, client } = actor.getSnapshot().context, generation = client.getSnapshot().generation, authority = client.getSnapshot().authorityRevision;
  return () => actor.getSnapshot().status === 'active' && actor.getSnapshot().context.control === control && client.getSnapshot().generation === generation && client.getSnapshot().authorityRevision === authority;
}

/** Local startup input interest may pass through exactly its own publication.
 * It is not an execution proof: native attachment/admission still gates I/O. */
export function draftSessionCurrent(actor: ShellNavigationActor, intent: Pending) {
  const { client } = actor.getSnapshot().context;
  return () => {
    const context = actor.getSnapshot().context;
    return actor.getSnapshot().status === 'active' && authorityCurrent(client, intent) && (context.pending === intent || context.publication?.intent === intent && context.publication.control === context.control);
  };
}
