import { assign, raise, setup, type ActorRefFrom } from 'xstate';
import type { AttachmentTarget, RuntimeClientSessionDeletionResult } from '../../../../protocol/app-server/v41';
import { emptyFacts, sameClaim, type Admission, type Attached, type LifecycleFacts, type Observation, type SessionLifecyclePort } from './port';

type Kind = 'open' | 'release' | 'switch' | 'delete' | 'recover' | 'inspect';
export interface Command {
  kind: Kind;
  work?: Promise<RuntimeClientSessionDeletionResult | undefined>;
  node?: string;
  revision?: string;
  current: () => boolean;
  attached?: (target: AttachmentTarget) => void;
  resolve(value?: RuntimeClientSessionDeletionResult): void;
  reject(error: unknown): void;
}
interface Operation {
  token: number; generation: number; revision: number; command: Command;
  node?: string; conversation?: string; target?: AttachmentTarget; stage: 'preparing' | 'request' | 'sent';
}
export interface LifecycleInput {
  id: string; readonly authority: object; generation: number; connected: boolean; port: SessionLifecyclePort;
  budget: { available(): boolean; take(): void; release(): void; identity(): number };
}
export interface LifecycleContext extends LifecycleInput {
  facts: LifecycleFacts;
  epoch: number;
  queue: readonly Operation[];
  active?: Operation;
  /** Native facts, not inferred from a browser desired selector. */
  selection?: { node: string; conversation: string };
  residency?: { conversation: string; generation: number };
  unresolved?: { kind: Kind; error: unknown };
  deleted: boolean;
  attachmentResult?: Attached;
}
type Intent = { type: 'OPEN' | 'RELEASE' | 'SWITCH_NODE' | 'DELETE' | 'RECOVER' | 'INSPECT_DELETION' | 'RECONNECT'; command: Command };
type Settled = { type: 'SETTLED'; token: number; outcome: { ok: true; value?: RuntimeClientSessionDeletionResult | { node: string; conversation: string } } | { ok: false; error: unknown; obsolete?: boolean } };
export type LifecycleEvent = Intent | Settled
  | { type: 'ADVANCE' }
  | { type: 'SENT' | 'SUBMIT'; token: number }
  | { type: 'SELECTION'; token: number; node: string; conversation: string }
  | { type: 'NODE'; token: number; node: string; conversation?: string }
  | { type: 'ATTACH_ACK'; token: number; result: Attached }
  | { type: 'TRANSPORT'; generation: number; connected: boolean }
  | { type: 'AUTHORITY_REPLACED' }
  | { type: 'RESTORE_INTENT' }
  | { type: 'ROUTE_CLOSED'; target: AttachmentTarget }
  | { type: 'OBSERVATION'; proof: Observation; status: 'resynchronizing' | 'attached' | 'stale'; error?: string };

export function observationCurrent(context: LifecycleContext, proof?: Observation): boolean {
  const f = context.facts;
  return !!proof && context.connected && f.attachmentObservation === proof && context.generation === proof.generation
    && f.attachmentIntent === 'wanted' && f.attachmentIntentRevision === proof.intentRevision
    && f.nodeId === proof.nodeId && f.attachmentNodeId === proof.nodeId
    && !f.deleting && sameClaim(f.target, proof.target);
}
export function controlCurrent(context: LifecycleContext, proof?: Observation): boolean {
  return context.facts.attachment === 'attached' && observationCurrent(context, proof);
}
export function switchOutstanding(context: LifecycleContext) { return context.active?.command.kind === 'switch' || context.queue.some(op => op.command.kind === 'switch'); }
function rejection(c: LifecycleContext, cmd: Command): string | undefined {
  const f = c.facts, kind = cmd.kind;
  if (c.deleted) return 'Session has been deleted.';
  if ((kind === 'open' || kind === 'delete') && switchOutstanding(c)) return 'Wait for the current Node switch to settle, then obtain fresh native evidence.';
  if (kind === 'switch' && (c.active || c.queue.length)) return 'Wait for the current attachment operation before switching nodes.';
  if (kind === 'switch' && !controlCurrent(c, f.attachmentObservation)) return 'Session is not authoritatively attached. Refresh or reconnect.';
  if ((kind === 'open' || kind === 'delete') && f.deleting) return 'Session deletion is already pending verification.';
  if (kind === 'recover' && (!f.deletionRecovery || f.recoveringDeletion)) return 'Observe committed deletion state before recovery.';
  if (kind === 'open' && cmd.node && f.nodeId && cmd.node !== f.nodeId
    && (f.attachmentIntent === 'wanted' && (c.active || c.queue.length) || f.target && !c.queue.some(op => op.command.kind === 'release') && c.active?.command.kind !== 'release')) return 'Use branch switching to open another node.';
  if (!c.connected && ['delete', 'recover', 'inspect'].includes(kind)) return 'Connect before requesting Session deletion or recovery.';
  if (!c.budget.available()) return 'Attachment operation capacity reached. Disconnect to release external claims.';
}
export function lifecycleAvailability(c: LifecycleContext) {
  const command = (kind: Kind): Command => ({ kind, current: () => true, resolve: () => {}, reject: () => {} });
  return { open: !rejection(c, command('open')), switchNode: !rejection(c, command('switch')), delete: !rejection(c, command('delete')) };
}
function owned(c: LifecycleContext, token: number) { return c.active?.token === token; }
function intentCurrent(c: LifecycleContext, op: Operation) {
  return owned(c, op.token) && c.connected && c.generation === op.generation
    && (op.command.kind !== 'open' && op.command.kind !== 'switch' || c.facts.attachmentIntent === 'wanted'
      && c.facts.attachmentIntentRevision === op.revision && !c.facts.deleting);
}
function revoke(f: LifecycleFacts): LifecycleFacts { return { ...f, attachmentObservation: undefined, attachmentIntentRevision: f.attachmentIntentRevision + 1 }; }
function deletion(c: LifecycleContext, result: RuntimeClientSessionDeletionResult): Partial<LifecycleContext> {
  if (result.status === 'deleted' || result.status === 'not_found') {
    for (const op of c.queue) { c.budget.release(); op.command.resolve(); }
    return { queue: [], deleted: true, epoch: c.budget.identity(), facts: { ...revoke(c.facts), attachmentIntent: 'released', attachment: 'detached', target: undefined, deleting: false, recoveringDeletion: false } };
  }
  if (result.status === 'committed_cleanup_pending' || result.status === 'committed_durability_uncertain') return { epoch: c.budget.identity(), facts: { ...revoke(c.facts), attachmentIntent: 'released', attachment: 'detached', target: undefined, deleting: true, deletionRecovery: result.status, deletionCommitted: result.status, recoveringDeletion: false, error: undefined } };
  return { facts: { ...c.facts, deleting: false, deletionRecovery: undefined, deletionCommitted: undefined, recoveringDeletion: false, error: undefined } };
}
/** Stable actor transaction delivery: intent changes never dispose a completion.
 * Only this function performs finite port I/O; all facts change through events. */
async function execute(self: { getSnapshot(): { context: LifecycleContext }; send(event: LifecycleEvent): void }, op: Operation) {
  const { port } = self.getSnapshot().context;
  const context = () => self.getSnapshot().context;
  // Caller freshness may synchronously send a lifecycle command. Read actor
  // authority again after it returns; never carry a context across that callback.
  const current = () => intentCurrent(context(), op) && op.command.current() && intentCurrent(context(), op);
  const settlement = () => owned(context(), op.token) && context().generation === op.generation;
  const submit = () => self.send({ type: 'SUBMIT', token: op.token });
  const proof: Admission = { current, validate: async () => current(), sent: () => self.send({ type: 'SENT', token: op.token }) };
  try {
    let value: RuntimeClientSessionDeletionResult | { node: string; conversation: string } | undefined;
    if (op.command.kind === 'open') {
      if (!current()) { self.send({ type: 'SETTLED', token: op.token, outcome: { ok: true } }); return; }
      const selected = op.node === undefined ? await port.resolveNode(current) : undefined;
      const node = op.node ?? selected!.node;
      if (!current()) { self.send({ type: 'SETTLED', token: op.token, outcome: { ok: true } }); return; }
      if (selected) self.send({ type: 'SELECTION', token: op.token, ...selected });
      self.send({ type: 'NODE', token: op.token, node });
      const f = context().facts;
      if (f.target) {
        if (f.attachmentNodeId !== node) throw new Error('The previous Node still owns an attachment. Release it before opening another node.');
        if (!observationCurrent(context(), f.attachmentObservation)) throw new Error('Release the retained attachment before opening again.');
        await port.refresh(f.target);
        if (current() && controlCurrent(context(), f.attachmentObservation)) op.command.attached?.(f.target);
      } else {
        const admission = await port.admit(current);
        const admitted = () => !!admission && admission.current() && current();
        if (admission && admitted()) {
          const conversation = context().facts.nodeConversationId ?? await port.conversation(node, admitted);
          if (admitted()) {
            self.send({ type: 'NODE', token: op.token, node, conversation });
            submit();
            const result = await port.attach(node, { current: admitted, sent: proof.sent, validate: async signal => {
              const valid = await admission.validate(signal);
              if (valid && admitted()) port.cold(admitted);
              return valid && admitted();
            } });
            if (settlement()) {
              self.send({ type: 'ATTACH_ACK', token: op.token, result });
              if (result.target.session_id !== context().id || result.target.conversation_id !== conversation || result.snapshot.conversation_id !== conversation) throw new Error('Mismatched attachment identity.');
              const observation = context().facts.attachmentObservation;
              if (controlCurrent(context(), observation)) {
                if (op.command.current() && controlCurrent(context(), observation)) op.command.attached?.(result.target);
                if (observationCurrent(context(), observation)) await port.observeAttached(result, () => observationCurrent(context(), observation));
              }
            }
          }
        }
      }
    } else if (op.command.kind === 'release') {
      if (op.target && settlement()) {
        submit();
        try { await port.detach(op.target, { ...proof, current: settlement }); }
        catch (error) { if (port.classify(error) !== 'stale-route') throw error; }
      }
    } else if (op.command.kind === 'switch') {
      submit();
      const result = await port.switchNode(op.target!, op.node!, proof);
      if (result.id !== context().id || result.active_node !== op.node) throw new Error('Mismatched switched Node identity.');
      value = { node: result.active_node, conversation: result.active_conversation_id };
    } else if (op.command.kind === 'delete') { submit(); value = await port.delete(op.command.revision!, proof); }
    else if (op.command.kind === 'recover') { submit(); value = await port.recover(proof); }
    else value = await port.inspectDeletion(current);
    self.send({ type: 'SETTLED', token: op.token, outcome: { ok: true, value } });
  } catch (error) {
    // Sample caller freshness outside the actor transition. The correlated
    // completion carries presentation obsolescence, never execution authority.
    let obsolete = false;
    if (op.command.kind === 'open') { try { obsolete = !current(); } catch { obsolete = true; } }
    self.send({ type: 'SETTLED', token: op.token, outcome: { ok: false, error, obsolete } });
  }
}

export const sessionLifecycleMachine = setup({
  types: { context: {} as LifecycleContext, input: {} as LifecycleInput, events: {} as LifecycleEvent },
  guards: {
    admitted: ({ context, event }) => 'command' in event && !rejection(context, event.command),
    correlated: ({ context, event }) => 'token' in event && owned(context, event.token),
    nextOpen: ({ context }) => !context.active && context.queue[0]?.command.kind === 'open',
    nextRelease: ({ context }) => !context.active && context.queue[0]?.command.kind === 'release',
    nextSwitch: ({ context }) => !context.active && context.queue[0]?.command.kind === 'switch',
    nextDelete: ({ context }) => !context.active && context.queue[0]?.command.kind === 'delete',
    nextRecovery: ({ context }) => !context.active && !!context.queue.length,
  },
  actions: {
    noWork: ({ event }) => { if ('command' in event) event.command.resolve(); },
    reject: ({ context, event }) => { if ('command' in event) event.command.reject(new Error(rejection(context, event.command))); },
    admit: assign(({ context: c, event }) => {
      if (!('command' in event)) return {};
      const cmd = event.type === 'RECONNECT' && c.facts.deleting ? { ...event.command, kind: 'inspect' as const } : event.command; let f = c.facts;
      if (cmd.kind === 'release') f = { ...revoke(f), attachmentIntent: 'released' };
      if (cmd.kind === 'switch' || cmd.kind === 'delete') f = revoke(f);
      if (cmd.kind === 'delete') f = { ...f, attachmentIntent: 'released', deleting: true, error: undefined };
      if (cmd.kind === 'recover') f = { ...f, recoveringDeletion: true, error: undefined };
      if (cmd.kind === 'open') f = { ...f, attachmentIntent: 'wanted', nodeId: cmd.node ?? f.nodeId,
        ...(cmd.node && cmd.node !== f.nodeId ? { nodeConversationId: undefined } : {}) };
      const op: Operation = { token: c.budget.identity(), generation: c.generation, revision: f.attachmentIntentRevision,
        command: cmd, node: cmd.node ?? f.nodeId, stage: 'preparing' };
      c.budget.take();
      return { facts: f, queue: [...c.queue, op] };
    }),
    begin: assign(({ context: c }) => {
      const op = c.queue[0]; if (!op) return {};
      const opening = op.command.kind === 'open' && !c.facts.target && c.facts.attachmentIntent === 'wanted' && c.facts.attachmentIntentRevision === op.revision;
      return { active: { ...op, target: c.facts.target }, queue: c.queue.slice(1),
        ...(opening ? { epoch: c.budget.identity(), facts: { ...c.facts, attachment: 'attaching' as const, error: undefined } } : {}) };
    }),
    execute: ({ context, self }) => { const op = context.active!; queueMicrotask(() => { void execute(self, op); }); },
    sent: assign(({ context, event }) => (event.type === 'SENT' || event.type === 'SUBMIT') && context.active ? { active: { ...context.active, stage: event.type === 'SENT' ? 'sent' as const : 'request' as const },
      ...(event.type === 'SENT' && ['switch', 'delete', 'recover'].includes(context.active.command.kind) ? { residency: undefined, selection: undefined } : {}) } : {}),
    selection: assign(({ context, event }) => event.type === 'SELECTION' && context.active?.generation === context.generation ? { selection: { node: event.node, conversation: event.conversation } } : {}),
    node: assign(({ context: c, event }) => event.type === 'NODE' && c.active && intentCurrent(c, c.active)
      ? { active: { ...c.active, node: event.node, conversation: event.conversation ?? c.active.conversation }, facts: { ...c.facts, nodeId: event.node, ...(event.conversation ? { nodeConversationId: event.conversation } : {}) } } : {}),
    attached: assign(({ context: c, event }) => {
      if (event.type !== 'ATTACH_ACK' || !c.active || c.active.generation !== c.generation) return {};
      const { result } = event, op = c.active;
      const valid = result.target.session_id === c.id && result.target.conversation_id === op.conversation && result.snapshot.conversation_id === op.conversation;
      const target = Object.freeze({ ...result.target });
      const observation = valid && intentCurrent(c, op) && c.facts.nodeId === op.node ? Object.freeze({ generation: c.generation, target, nodeId: op.node!, intentRevision: op.revision }) : undefined;
      // A fresh native claim establishes current ownership; historical transport
      // uncertainty remains in the client diagnostic owner.
      return { unresolved: valid ? undefined : c.unresolved, attachmentResult: observation ? result : undefined, facts: { ...c.facts, target, attachmentNodeId: op.node, attachmentObservation: observation, attachment: 'attached' as const }, residency: { conversation: result.target.conversation_id, generation: c.generation } };
    }),
    settle: assign(({ context: c, event }) => {
      if (event.type !== 'SETTLED' || !c.active) return {};
      if (c.active.generation !== c.generation) {
        if (!event.outcome.ok && c.port.classify(event.outcome.error) === 'uncertain') return { unresolved: { kind: c.active.command.kind, error: event.outcome.error }, facts: { ...c.facts, attachmentIntent: 'released' } };
        if (!event.outcome.ok && c.active.command.kind === 'delete' && c.port.classify(event.outcome.error) === 'unsent') return { facts: { ...c.facts, deleting: false } };
        return {};
      }
      const op = c.active, kind = op.command.kind, f = c.facts;
      if (!event.outcome.ok) {
        const error = event.outcome.error, classification = c.port.classify(error);
        const unresolved = classification === 'uncertain' ? { kind, error } : c.unresolved;
        if (kind === 'switch') return { unresolved, facts: { ...f, attachmentObservation: undefined, attachment: 'stale', error: String(error), ...(classification !== 'unsent' ? { attachmentIntent: 'released', nodeId: undefined, nodeConversationId: undefined } : {}) } };
        if (kind === 'delete' && classification === 'unsent') return { facts: { ...f, deleting: false, error: String(error) } };
        if (kind === 'recover') return { unresolved, facts: { ...f, recoveringDeletion: false, error: String(error), ...(classification === 'uncertain' ? { deletionRecovery: undefined } : {}) } };
        if (kind === 'open') {
          const absent = !f.target && (classification === 'unsent' || classification === 'refused' || op.stage === 'preparing');
          if (absent && (event.outcome.obsolete || !intentCurrent(c, op))) return { facts: { ...f, attachment: c.unresolved ? 'stale' : 'detached', error: undefined } };
          return { unresolved, facts: { ...f, attachment: 'error', error: String(error) } };
        }
        return { unresolved, facts: { ...f, error: String(error) } };
      }
      const value = event.outcome.value;
      if (kind === 'switch' && value && 'node' in value) return { selection: value, residency: { conversation: value.conversation, generation: c.generation },
        facts: { ...f, target: undefined, attachmentNodeId: undefined, attachmentObservation: undefined, attachment: 'detached', nodeId: value.node, nodeConversationId: value.conversation, error: undefined } };
      if ((kind === 'delete' || kind === 'recover' || kind === 'inspect') && value && 'status' in value) return deletion(c, value);
      if (kind === 'release' && (!op.target || sameClaim(op.target, f.target) || !f.target)) return { facts: { ...f, target: undefined, attachmentObservation: undefined, attachment: c.unresolved && !op.target ? 'stale' : 'detached', error: undefined } };
      if (kind === 'open' && !f.target && f.attachment === 'attaching') return { facts: { ...f, attachment: c.unresolved ? 'stale' : 'detached', error: undefined } };
      return {};
    }),
    finish: ({ context, event }) => {
      if (event.type !== 'SETTLED' || !context.active) return;
      context.budget.release();
      if (!event.outcome.ok) context.active.command.reject(event.outcome.error);
      else context.active.command.resolve(event.outcome.value && 'status' in event.outcome.value ? event.outcome.value : undefined);
    },
    clear: assign({ active: () => undefined }),
    transport: assign(({ context: c, event }) => {
      if (event.type !== 'TRANSPORT' || event.generation === c.generation && event.connected === c.connected) return {};
      if (event.connected) return { generation: event.generation, connected: true };
      for (const op of c.queue) { c.budget.release(); op.command.resolve(); }
      const switching = switchOutstanding(c);
      const unsent = c.active && c.active.stage === 'preparing';
      const unsentDelete = c.queue.some(op => op.command.kind === 'delete') || unsent && c.active?.command.kind === 'delete';
      if (unsent) { c.budget.release(); c.active!.command.resolve(); }
      return { generation: event.generation, connected: false, queue: [], residency: undefined, ...(unsent ? { active: undefined } : {}),
        facts: { ...revoke(c.facts), target: undefined, recoveringDeletion: false, ...(unsentDelete ? { deleting: false } : {}),
          ...(c.facts.recoveringDeletion ? { deletionRecovery: undefined } : {}),
          ...(switching ? { attachmentIntent: 'released', nodeId: undefined, nodeConversationId: undefined } : {}),
          attachment: c.facts.target || c.active ? 'stale' : c.facts.attachment } };
    }),
    routeClosed: assign(({ context: c, event }) => event.type === 'ROUTE_CLOSED' && sameClaim(c.facts.target, event.target)
      ? { facts: { ...revoke(c.facts), target: undefined, attachment: 'stale', error: c.facts.deleting ? undefined : 'Session connection closed. Open the Session to inspect its current state.' } } : {}),
    observation: assign(({ context: c, event }) => event.type === 'OBSERVATION' && observationCurrent(c, event.proof)
      ? { facts: { ...c.facts, attachment: event.status, error: event.error } } : {}),
    releaseWithoutCapacity: assign(({ context }) => context.deleted ? {} : { facts: { ...revoke(context.facts), attachmentIntent: 'released' as const, attachment: context.facts.target ? 'stale' as const : context.facts.attachment } }),
    restore: assign(({ context }) => ({ facts: { ...context.facts, attachmentIntent: 'wanted' as const } })),
    retire: assign(({ context: c }) => {
      for (const op of c.queue) { c.budget.release(); op.command.resolve(); }
      const unsent = c.active?.stage === 'preparing';
      if (unsent) { c.budget.release(); c.active!.command.resolve(); }
      return { connected: false, queue: [], ...(unsent ? { active: undefined } : {}), facts: { ...revoke(c.facts), attachmentIntent: 'released' as const, target: undefined, attachment: 'stale' as const } };
    }),
  },
}).createMachine({
  id: 'sessionLifecycle',
  context: ({ input }) => ({ ...input, facts: emptyFacts, epoch: input.budget.identity(), queue: [], deleted: false }),
  initial: 'idle',
  on: {
    OPEN: [{ guard: 'admitted', actions: ['admit', raise({ type: 'ADVANCE' })] }, { actions: 'reject' }],
    RELEASE: [{ guard: 'admitted', actions: ['admit', raise({ type: 'ADVANCE' })] }, { actions: ['releaseWithoutCapacity', 'reject'] }],
    SWITCH_NODE: [{ guard: 'admitted', actions: ['admit', raise({ type: 'ADVANCE' })] }, { actions: 'reject' }],
    DELETE: [{ guard: 'admitted', actions: ['admit', raise({ type: 'ADVANCE' })] }, { actions: 'reject' }],
    RECOVER: [{ guard: 'admitted', actions: ['admit', raise({ type: 'ADVANCE' })] }, { actions: 'reject' }],
    RECONNECT: [
      { guard: ({ context }) => !!context.facts.deleting && context.budget.available(), actions: ['admit', raise({ type: 'ADVANCE' })] },
      { guard: ({ context, event }) => context.facts.attachmentIntent === 'wanted' && !context.unresolved && context.facts.attachment !== 'error' && 'command' in event && !rejection(context, event.command), actions: ['admit', raise({ type: 'ADVANCE' })] },
      { actions: 'noWork' },
    ],
    INSPECT_DELETION: [{ guard: 'admitted', actions: ['admit', raise({ type: 'ADVANCE' })] }, { actions: 'reject' }],
    ADVANCE: [
      { guard: 'nextOpen', target: '.opening' }, { guard: 'nextRelease', target: '.releasing' },
      { guard: 'nextSwitch', target: '.switching' }, { guard: 'nextDelete', target: '.deleting' },
      { guard: 'nextRecovery', target: '.recovering' },
    ],
    SENT: { guard: 'correlated', actions: 'sent' },
    SUBMIT: { guard: 'correlated', actions: 'sent' },
    NODE: { guard: 'correlated', actions: 'node' },
    SELECTION: { guard: 'correlated', actions: 'selection' },
    ATTACH_ACK: { guard: 'correlated', actions: 'attached' },
    SETTLED: { guard: 'correlated', actions: ['settle', 'finish', 'clear'], target: '.idle' },
    TRANSPORT: [ { guard: ({ context, event }) => event.type === 'TRANSPORT' && !event.connected && !!context.active && context.active.stage === 'preparing', actions: 'transport', target: '.idle' }, { actions: 'transport' } ],
    ROUTE_CLOSED: { actions: 'routeClosed' },
    OBSERVATION: { actions: 'observation' },
    RESTORE_INTENT: { actions: 'restore' },
    AUTHORITY_REPLACED: { actions: 'retire', target: '.retired' },
  },
  states: {
    idle: { entry: raise({ type: 'ADVANCE' }), initial: 'classify',
      on: {
        OBSERVATION: { actions: 'observation', target: '.classify' },
        ROUTE_CLOSED: { actions: 'routeClosed', target: '.classify' },
        TRANSPORT: { actions: 'transport', target: '.classify' },
      },
      states: {
        classify: { always: [
          { guard: ({ context }) => context.deleted, target: '#sessionLifecycle.retired' },
          { guard: ({ context }) => controlCurrent(context, context.facts.attachmentObservation), target: 'attached' },
          { guard: ({ context }) => !!context.facts.target || !!context.facts.deleting || !!context.unresolved || context.facts.attachment === 'stale' || context.facts.attachment === 'error', target: 'unresolved' },
          { target: 'detached' },
        ] },
        detached: {}, attached: {}, unresolved: {},
      },
    },
    opening: { entry: ['begin', 'execute'] }, releasing: { entry: ['begin', 'execute'] },
    switching: { entry: ['begin', 'execute'] }, deleting: { entry: ['begin', 'execute'] }, recovering: { entry: ['begin', 'execute'] },
    // Retired actors accept only their already-owned transaction completion.
    retired: { on: { OPEN: { actions: 'reject' }, RELEASE: { actions: 'reject' }, SWITCH_NODE: { actions: 'reject' }, DELETE: { actions: 'reject' }, RECOVER: { actions: 'reject' },
      SETTLED: { guard: 'correlated', actions: ['finish', 'clear'] }, ADVANCE: {}, SENT: {}, SUBMIT: {}, NODE: {}, SELECTION: {}, ATTACH_ACK: {}, TRANSPORT: {}, RESTORE_INTENT: {}, RECONNECT: { actions: 'reject' }, INSPECT_DELETION: { actions: 'reject' }, ROUTE_CLOSED: {}, OBSERVATION: {} } },
  },
});
export type LifecycleActor = ActorRefFrom<typeof sessionLifecycleMachine>;
