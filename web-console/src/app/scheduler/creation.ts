import { parseAbsolute } from '@internationalized/date';
import type { ScheduleCommand, ScheduleTiming, SessionModelConfig } from '../../../../protocol/app-server/v79';
import type { AppServerClient } from '../../client/app-server';
import type { OperationAdmission } from '../../client/contracts';
import { isOutcomeUncertain, RequestNotDispatched, RpcFailure } from '../../client/errors';
import { sameEndpoint } from '../../workspaces/endpoint';
import type { ProductHostWorkspaces } from '../../workspaces/host';
import { parseCronExpression } from './cron';
import { schedulerModel, type TaskRef } from './model';

export interface TaskCreationDraft {
  workspaceId: string;
  title: string;
  text: string;
  timing: ScheduleTiming;
  model?: SessionModelConfig;
}
export type TaskCreationError = 'invalid' | 'disconnected' | 'workspace_changed' | 'authority_changed'
  | 'create_failed' | 'create_uncertain' | 'schedule_failed' | 'schedule_uncertain';
export interface TaskCreationState {
  phase: 'idle' | 'creating' | 'scheduling' | 'failed' | 'uncertain' | 'created';
  locked: boolean;
  retryable: boolean;
  task?: TaskRef;
  errorKind?: TaskCreationError;
  draft?: Readonly<TaskCreationDraft>;
}
interface Operation {
  draft: TaskCreationDraft;
  endpoint: string;
  authorityId: string;
  authorityRevision: number | undefined;
  hostAuthority?: string;
  cwd?: string;
  session?: { session_id: string; node_id: string };
  command: ScheduleCommand;
  createUncertain?: boolean;
  scheduleUncertain?: boolean;
}
class CreationFailure extends Error {
  constructor(readonly kind: TaskCreationError) { super(kind); }
}
function validDraft(draft: TaskCreationDraft) {
  if (!draft.workspaceId.trim() || !draft.title.trim() || !draft.text.trim()) return false;
  if (Array.from(draft.title).length > 120 || new TextEncoder().encode(draft.title).length > 480
    || new TextEncoder().encode(draft.text).length > 16_384) return false;
  const timing = draft.timing;
  switch (timing.type) {
    case 'at':
      if (!/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d{1,9})?(?:Z|[+-]\d\d:\d\d)$/.test(timing.at)) return false;
      try { return parseAbsolute(timing.at, 'UTC').toDate().getTime() > Date.now(); } catch { return false; }
    case 'after': return Number.isInteger(timing.seconds) && timing.seconds > 0 && timing.seconds <= 4_294_967_295;
    case 'interval': return Number.isInteger(timing.every_seconds) && timing.every_seconds >= 60 && timing.every_seconds <= 4_294_967_295;
    case 'cron':
      if (timing.expression.length > 256 || timing.time_zone.length > 128 || !parseCronExpression(timing.expression)) return false;
      try { new Intl.DateTimeFormat('en', { timeZone: timing.time_zone }); return !!timing.time_zone.trim(); } catch { return false; }
    default: return false;
  }
}

/** Owns one native creation until its outcome is known. It never attaches or
 * sends a Human input, and an ambiguous Session create is never replayed. */
export class TaskCreation {
  private listeners = new Set<() => void>();
  private snapshot: TaskCreationState = { phase: 'idle', locked: false, retryable: false };
  private operation?: Operation;
  private pending = false;
  constructor(private client: AppServerClient, private host: ProductHostWorkspaces) {}
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  getSnapshot = () => this.snapshot;
  private publish(state: TaskCreationState) {
    this.snapshot = { ...state, ...(state.locked && this.operation ? { draft: structuredClone(this.operation.draft) } : {}) };
    for (const listener of this.listeners) listener();
  }
  /** Explicit discard may leave an empty Session, including an unacknowledged
   * create: without its identity no schedule was dispatched. An ambiguous
   * schedule must retain its command identity until its outcome is resolved. */
  reset = () => {
    if (this.pending || this.snapshot.phase === 'uncertain' && !(this.operation?.createUncertain && !this.operation.session)) return false;
    this.operation = undefined; this.publish({ phase: 'idle', locked: false, retryable: false }); return true;
  };
  isCurrentAuthority = () => !this.operation || this.sameAuthority(this.operation);
  private sameAuthority(operation: Operation) {
    const state = this.client.getSnapshot();
    return sameEndpoint(state.endpoint, operation.endpoint) && state.authorityId === operation.authorityId
      && state.authorityRevision === operation.authorityRevision;
  }
  private admission(operation: Operation, generation: number, sent: () => void): OperationAdmission {
    const current = () => this.sameAuthority(operation) && this.client.getSnapshot().connection === 'connected'
      && this.client.getSnapshot().generation === generation;
    return { current, sent, validate: async signal => {
      const catalog = await this.host.listWorkspaces(signal);
      if (!current()) throw new CreationFailure('authority_changed');
      if (!sameEndpoint(catalog.endpoint, operation.endpoint) || catalog.authorityId !== operation.hostAuthority)
        throw new CreationFailure('authority_changed');
      if (!catalog.workspaces.some(workspace => workspace.id === operation.draft.workspaceId)) throw new CreationFailure('workspace_changed');
      const resolved = await this.host.resolveWorkspace(operation.draft.workspaceId, operation.endpoint);
      if (resolved.cwd !== operation.cwd) throw new CreationFailure('workspace_changed');
      const [location] = await this.host.classifyLocations([resolved.cwd], operation.endpoint, catalog.authorityId, signal);
      if (!location?.authorized) throw new CreationFailure('workspace_changed');
      return current();
    } };
  }
  submit = async (draft: TaskCreationDraft): Promise<TaskRef | undefined> => {
    if (this.pending || this.snapshot.locked) return;
    if (!validDraft(draft)) { this.publish({ phase: 'failed', locked: false, retryable: false, errorKind: 'invalid' }); return; }
    const state = this.client.getSnapshot();
    if (state.connection !== 'connected' || !state.endpoint || !state.authorityId) {
      this.publish({ phase: 'failed', locked: false, retryable: false, errorKind: 'disconnected' }); return;
    }
    const copy = structuredClone(draft);
    const operation: Operation = { draft: copy, endpoint: state.endpoint, authorityId: state.authorityId, authorityRevision: state.authorityRevision,
      command: { command_id: crypto.randomUUID(), mutation: { type: 'create', title: copy.title, text: copy.text, timing: copy.timing } } };
    this.operation = operation; this.pending = true;
    this.publish({ phase: 'creating', locked: true, retryable: false });
    let sent = false;
    try {
      const catalog = await this.host.listWorkspaces();
      if (!sameEndpoint(catalog.endpoint, operation.endpoint) || !catalog.workspaces.some(workspace => workspace.id === copy.workspaceId)) throw new CreationFailure('workspace_changed');
      operation.hostAuthority = catalog.authorityId;
      operation.cwd = (await this.host.resolveWorkspace(copy.workspaceId, operation.endpoint)).cwd;
      const [location] = await this.host.classifyLocations([operation.cwd], operation.endpoint, catalog.authorityId);
      if (!location?.authorized) throw new CreationFailure('workspace_changed');
      const admission = this.admission(operation, state.generation, () => { sent = true; });
      await this.client.request({ method: 'thread/start', params: { settings: { cwd: operation.cwd, ...(copy.model ? { model: copy.model } : {}) } } }, 'session_transition', result => {
        operation.session = { session_id: result.session.id, node_id: result.session.active_node };
      }, admission);
      return await this.schedule(operation, state.generation);
    } catch (cause) {
      if (!operation.session) operation.createUncertain = isOutcomeUncertain(cause) || sent && !(cause instanceof RpcFailure) && !(cause instanceof RequestNotDispatched);
      this.fail(operation, cause, operation.session ? 'schedule_failed' : 'create_failed');
      return this.snapshot.task;
    } finally { this.pending = false; }
  };
  retry = async (): Promise<TaskRef | undefined> => {
    const operation = this.operation;
    if (this.pending || !this.snapshot.retryable || !operation?.session || operation.createUncertain) return;
    this.pending = true;
    try { return await this.schedule(operation, this.client.getSnapshot().generation); }
    catch (cause) { this.fail(operation, cause, 'schedule_failed'); return this.snapshot.task; }
    finally { this.pending = false; }
  };
  private async schedule(operation: Operation, generation: number): Promise<TaskRef | undefined> {
    if (!this.sameAuthority(operation)) throw new CreationFailure('authority_changed');
    if (this.client.getSnapshot().connection !== 'connected') throw new CreationFailure('disconnected');
    this.publish({ phase: 'scheduling', locked: true, retryable: false });
    let sent = false;
    try {
      const result = await this.client.request({ method: 'schedule/command', params: { ...operation.session!, command: operation.command } }, 'schedule_command', response => {
        if (response.result.status === 'applied') {
          const task = { ...operation.session!, id: response.result.receipt.id };
          operation.scheduleUncertain = false;
          this.publish({ phase: 'created', locked: true, retryable: false, task });
        }
      }, this.admission(operation, generation, () => { sent = true; }));
      if (result.result.status !== 'applied') throw new CreationFailure('schedule_failed');
    } catch (cause) {
      // A decoded committed receipt remains authoritative even if a later local
      // continuation or carrier fails. No failure may erase this success.
      if (!this.snapshot.task) {
        operation.scheduleUncertain ||= isOutcomeUncertain(cause) || sent && !(cause instanceof RpcFailure) && !(cause instanceof RequestNotDispatched) && !(cause instanceof CreationFailure);
        throw cause;
      }
    }
    if (this.sameAuthority(operation) && this.client.getSnapshot().connection === 'connected') {
      const model = schedulerModel(this.client), stop = model.watchAll();
      void model.refresh().finally(stop);
    }
    return this.snapshot.task;
  }
  private fail(operation: Operation, cause: unknown, fallback: TaskCreationError) {
    if (this.snapshot.task) return;
    const uncertain = operation.createUncertain || operation.scheduleUncertain;
    const failure = cause instanceof CreationFailure ? cause : cause instanceof RequestNotDispatched && cause.cause instanceof CreationFailure ? cause.cause : undefined;
    const errorKind = operation.createUncertain ? 'create_uncertain' : operation.scheduleUncertain ? 'schedule_uncertain' : failure?.kind ?? fallback;
    this.publish({ phase: uncertain ? 'uncertain' : 'failed', locked: !!operation.session || !!uncertain,
      retryable: !!operation.session && !operation.createUncertain && this.sameAuthority(operation), errorKind });
  }
}

const owners = new WeakMap<AppServerClient, { host: ProductHostWorkspaces; authority: string; owner: TaskCreation }>();
/** Route lifetime never owns an admitted creation. Reconnect retains it;
 * selecting another native authority starts a separate creation owner. */
export function taskCreation(client: AppServerClient, host: ProductHostWorkspaces) {
  const state = client.getSnapshot();
  const authority = JSON.stringify([state.endpoint, state.authorityId, state.authorityRevision]);
  let entry = owners.get(client);
  if (!entry || entry.host !== host || entry.authority !== authority) {
    entry = { host, authority, owner: new TaskCreation(client, host) };
    owners.set(client, entry);
  }
  return entry.owner;
}
