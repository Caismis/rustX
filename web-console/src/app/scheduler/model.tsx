import { useMemo, useSyncExternalStore } from 'react';
import type { ScheduleHeader, ScheduleSnapshot, ScheduleMutation, ScheduleCommand, ScheduleCommandResult, ScheduledWorkflowRecovery, SessionSummary, ScheduleCatalogCursor } from '../../../../protocol/app-server/v62';
import type { AppServerClient } from '../../client/app-server';
import { isOutcomeUncertain, RpcFailure, RequestNotDispatched } from '../../client/errors';
import { TaskEditor } from './editor';

export interface TaskScope { session_id: string; node_id: string }
export interface TaskRef extends TaskScope { id: ScheduleHeader['id'] }
export const taskKey = (ref: TaskRef) => JSON.stringify([ref.session_id, ref.node_id, ref.id.conversation_id, ref.id.ordinal]);
export const taskRef = (task: Pick<ScheduleHeader, 'binding' | 'id'>): TaskRef => ({ ...task.binding, id: task.id });
const scopeKey = (scope: TaskScope) => JSON.stringify([scope.session_id, scope.node_id]);
export interface TaskCatalog {
  records: readonly ScheduleHeader[];
  sessions: ReadonlyMap<string, SessionSummary>;
  loading: boolean;
  connected: boolean;
  issue: boolean;
  revision: number;
  noticeId: number;
  uncertain: ReadonlySet<string>;
  notice?: 'updated' | 'deleted' | 'failed';
}
export class TaskChangeError extends Error {
  constructor(readonly kind: 'conflict' | 'uncertain' | 'invalid' | 'failed', readonly detail?: string) { super(kind); }
}
/** One observable query projection. All task authority remains in the native
 * exact-node API. Closing a view retires its reads, never an admitted mutation. */
export class SchedulerModel {
  private listeners = new Set<() => void>();
  private records = new Map<string, ScheduleHeader>();
  private sessions = new Map<string, SessionSummary>();
  private nodes = new Map<string, number>();
  private watchedSessions = new Map<string, number>();
  private global = 0;
  private epoch = 0;
  private authority?: string;
  private generation = -1;
  private revision = 0;
  private loading = 0;
  private connected = false;
  private notice?: TaskCatalog['notice'];
  private noticeId = 0;
  private failures = new Set<string>();
  private changes = new Map<string, { digest: string; command: ScheduleCommand; pending: boolean }>();
  private snapshot: TaskCatalog = { records: [], sessions: new Map(), loading: false, connected: false, issue: false, revision: 0, noticeId: 0, uncertain: new Set() };
  private flights = new Map<string, { promise: Promise<void>; again: boolean }>();
  private readers = 0;
  private waiting: Array<() => void> = [];
  private stopped = true;
  private stopFollowing?: () => void;
  private mutations = 0;
  private editors = new Map<string, TaskEditor>();
  constructor(readonly client: AppServerClient) {}
  private editorKey(ref: TaskRef) {
    const state = this.client.getSnapshot();
    return JSON.stringify([state.endpoint, state.authorityId, taskKey(ref)]);
  }
  editor(value: ScheduleSnapshot) {
    const key = this.editorKey(taskRef(value));
    let editor = this.editors.get(key);
    if (!editor) {
      const owned = new TaskEditor(value, () => { if (this.editors.get(key) === owned) this.editors.delete(key); });
      this.editors.set(key, owned); editor = owned;
    }
    return editor;
  }
  subscribe = (listener: () => void) => {
    this.listeners.add(listener); this.ensureFollowing();
    return () => { this.listeners.delete(listener); this.releaseFollowing(); };
  };
  getSnapshot = () => this.snapshot;
  private ensureFollowing() {
    if (!this.stopped) return false;
    this.flights.clear(); this.stopFollowing = this.start(); return true;
  }
  private releaseFollowing() {
    if (!this.listeners.size && !this.global && !this.nodes.size && !this.watchedSessions.size && !this.mutations) {
      this.stopFollowing?.(); this.stopFollowing = undefined;
    }
  }
  dispose() { this.stopFollowing?.(); this.stopFollowing = undefined; this.editors.clear(); }
  private publish() {
    this.snapshot = { records: [...this.records.values()], sessions: new Map(this.sessions), connected: this.connected,
      loading: this.loading > 0, issue: this.failures.size > 0, revision: ++this.revision, notice: this.notice, noticeId: this.noticeId,
      uncertain: new Set([...this.changes].filter(([, operation]) => !operation.pending).map(([key]) => key)) };
    for (const listener of this.listeners) listener();
  }
  start() {
    this.stopped = false;
    let signature = '';
    const follow = () => {
      const state = this.client.getSnapshot();
      const connected = state.connection === 'connected';
      const authority = connected && state.authorityId ? JSON.stringify([state.endpoint, state.authorityId]) : this.authority;
      const reset = state.generation !== this.generation || authority !== this.authority;
      if (reset) {
        this.epoch++;
        if (this.authority && authority !== this.authority && connected) { this.records.clear(); this.sessions.clear(); this.changes.clear(); this.failures.clear(); this.editors.clear(); }
        this.generation = state.generation; this.authority = authority;
        this.flights.clear();
      }
      const changed = connected !== this.connected;
      this.connected = connected;
      const next = JSON.stringify([state.generation, connected, state.sessions.map(row => [row.id, row.updated_at, row.active_node])]);
      if (signature !== next) {
        const reconnect = !signature || changed || reset;
        signature = next;
        for (const row of state.sessions) this.sessions.set(row.id, row);
        if (connected && reconnect) this.refreshObserved();
        this.publish();
      } else if (changed) this.publish();
    };
    const stop = this.client.subscribe(follow);
    const stopChanges = this.client.subscribeScheduleChanges(() => this.refreshObserved());
    const focus = () => { if (document.visibilityState !== 'hidden') this.refreshObserved(); };
    if (typeof window !== 'undefined') { window.addEventListener('focus', focus); document.addEventListener('visibilitychange', focus); }
    follow();
    return () => {
      this.stopped = true; this.epoch++; stop(); stopChanges();
      if (typeof window !== 'undefined') { window.removeEventListener('focus', focus); document.removeEventListener('visibilitychange', focus); }
    };
  }
  private capture() {
    const epoch = this.epoch, state = this.client.getSnapshot();
    return () => !this.stopped && this.connected && this.epoch === epoch && this.client.getSnapshot().generation === state.generation;
  }
  private async read<T>(work: () => Promise<T>, current: () => boolean): Promise<T> {
    if (this.readers >= 2) await new Promise<void>(resolve => this.waiting.push(resolve));
    else this.readers++;
    try {
      if (!current()) throw new Error('Task read retired');
      return await work();
    } finally { const next = this.waiting.shift(); if (next) next(); else this.readers--; }
  }
  private refreshObserved() {
    if (!this.connected || this.stopped) return;
    if (this.global) { void this.refresh(); return; }
    for (const id of this.watchedSessions.keys()) void this.refreshSession(id);
    for (const key of this.nodes.keys()) {
      const [session_id, node_id] = JSON.parse(key) as [string, string];
      if (this.watchedSessions.has(session_id)) continue;
      void this.refreshNode({ session_id, node_id });
    }
  }
  watchAll() { this.global++; if (!this.ensureFollowing()) void this.refresh(); return () => { this.global--; this.releaseFollowing(); }; }
  watchSession(id: string) {
    this.watchedSessions.set(id, (this.watchedSessions.get(id) ?? 0) + 1);
    if (!this.ensureFollowing()) void this.refreshSession(id);
    return () => { const count = (this.watchedSessions.get(id) ?? 1) - 1; if (count) this.watchedSessions.set(id, count); else this.watchedSessions.delete(id); this.releaseFollowing(); };
  }
  watchNode(scope: TaskScope) {
    const key = scopeKey(scope);
    this.nodes.set(key, (this.nodes.get(key) ?? 0) + 1);
    if (!this.ensureFollowing()) void this.refreshNode(scope);
    return () => { const count = (this.nodes.get(key) ?? 1) - 1; if (count) this.nodes.set(key, count); else this.nodes.delete(key); this.releaseFollowing(); };
  }
  private flight(key: string, work: (current: () => boolean) => Promise<void>) {
    if (!this.connected || this.stopped) return Promise.resolve();
    const held = this.flights.get(key);
    if (held) { held.again = true; return held.promise; }
    const current = this.capture();
    this.loading++; this.publish();
    const entry = { promise: Promise.resolve(), again: false };
    const promise = (async () => {
      do {
        entry.again = false;
        try { await work(current); }
        catch { if (current()) this.failures.add(key); }
      } while (entry.again && current());
    })().finally(() => {
      this.loading--;
      if (this.flights.get(key) === entry) this.flights.delete(key);
      this.publish();
    });
    entry.promise = promise;
    this.flights.set(key, entry);
    return promise;
  }
  private async catalog(session: string | undefined, current: () => boolean, interested: () => boolean) {
    const fresh = new Map<string, ScheduleHeader>(), unavailable = new Set<string>();
    let after: ScheduleCatalogCursor | null | undefined;
    do {
      const previous = after;
      const response = await this.read(() => this.client.request({ method: 'schedule/catalog', params: { session_id: session, after, limit: 100 } }, 'schedule_catalog', undefined, current), current);
      if (!current() || !interested()) return;
      for (const entry of response.page.entries) { fresh.set(taskKey(taskRef(entry.schedule)), entry.schedule); this.sessions.set(entry.session.id, entry.session); }
      for (const binding of response.page.unavailable) unavailable.add(scopeKey(binding));
      after = response.page.next;
      if (after && previous && JSON.stringify(after) === JSON.stringify(previous)) throw Error('Invalid task catalog page');
    } while (after && current() && interested());
    if (!current() || !interested()) return;
    for (const [key, row] of this.records) if ((!session || row.binding.session_id === session) && !unavailable.has(scopeKey(row.binding)) && !fresh.has(key)) this.records.delete(key);
    for (const [key, row] of fresh) this.records.set(key, row);
    const key = session ? 'session:' + session : 'catalog';
    if (unavailable.size) this.failures.add(key); else this.failures.delete(key);
  }
  refresh = () => this.flight('catalog', current => this.catalog(undefined, current, () => this.global > 0));
  refreshSession(id: string) {
    return this.flight('session:' + id, current => this.catalog(id, current, () => this.global > 0 || this.watchedSessions.has(id)));
  }
  refreshNode(scope: TaskScope, force = false) {
    return this.flight('node:' + scopeKey(scope), async current => {
      const interested = () => current() && (force || this.global > 0 || this.nodes.has(scopeKey(scope)) || this.watchedSessions.has(scope.session_id));
      const fresh = new Map<string, ScheduleHeader>();
      let after: string | null | undefined = '0';
      while (after != null && interested()) {
        const page = await this.read(() => this.client.request({ method: 'schedule/list', params: { ...scope, after_ordinal: after!, limit: 100 } }, 'schedules', undefined, current), current);
        if (!current()) return;
        for (const row of page.schedules) fresh.set(taskKey(taskRef(row)), row);
        if (page.next_ordinal != null && BigInt(page.next_ordinal) <= BigInt(after)) throw new Error('Invalid task page');
        after = page.next_ordinal;
      }
      if (!interested()) return;
      for (const [key, row] of this.records) if (row.binding.session_id === scope.session_id && row.binding.node_id === scope.node_id) this.records.delete(key);
      for (const [key, row] of fresh) this.records.set(key, row);
      this.failures.delete('node:' + scopeKey(scope));
    });
  }
  async get(ref: TaskRef, interested = () => true): Promise<ScheduleSnapshot | undefined> {
    const captured = this.capture(), current = () => captured() && interested();
    const result = await this.read(() => this.client.request({ method: 'schedule/get', params: { session_id: ref.session_id, node_id: ref.node_id, id: ref.id } }, 'schedule', undefined, current), current);
    if (!current()) return;
    const pending = this.changes.get(taskKey(ref));
    if (pending && (!result.schedule || 'expected_revision' in pending.command.mutation && pending.command.mutation.expected_revision !== result.schedule.revision)) this.changes.delete(taskKey(ref));
    if (!result.schedule) this.records.delete(taskKey(ref));
    else if (this.records.get(taskKey(ref))?.revision !== result.schedule.revision) await this.refreshNode(ref, true);
    this.publish();
    return result.schedule ?? undefined;
  }
  async change(ref: TaskRef, mutation: ScheduleMutation): Promise<ScheduleCommandResult> {
    this.mutations++; this.ensureFollowing();
    try { return await this.applyChange(ref, mutation); }
    finally { this.mutations--; this.releaseFollowing(); }
  }
  async retry(ref: TaskRef): Promise<ScheduleCommandResult> {
    const operation = this.changes.get(taskKey(ref));
    if (!operation || operation.pending) throw new TaskChangeError('invalid');
    const editor = this.editors.get(this.editorKey(ref));
    const result = await this.change(ref, operation.command.mutation);
    if (result.status === 'applied') editor?.recovered(result.receipt, operation.command.command_id);
    return result;
  }
  private async applyChange(ref: TaskRef, mutation: ScheduleMutation): Promise<ScheduleCommandResult> {
    const current = this.capture();
    if (!current()) throw new TaskChangeError('failed');
    const key = taskKey(ref), digest = JSON.stringify(mutation), previous = this.changes.get(key);
    if (previous && (previous.pending || previous.digest !== digest)) throw new TaskChangeError('uncertain');
    const operation = previous ?? { digest, command: { command_id: crypto.randomUUID(), mutation }, pending: false };
    operation.pending = true; this.changes.set(key, operation);
    if (!previous) this.editors.get(this.editorKey(ref))?.dispatched(operation.command.command_id);
    let acknowledged = false;
    try {
      const result = await this.client.request({ method: 'schedule/command', params: { session_id: ref.session_id, node_id: ref.node_id,
        command: operation.command } }, 'schedule_command', () => { acknowledged = true; }, current);
      if (!current()) throw new TaskChangeError('uncertain');
      if (result.result.status === 'conflict') {
        this.changes.delete(key);
        await this.refreshNode(ref, true);
        throw new TaskChangeError('conflict');
      }
      this.changes.delete(key);
      if (!['applied', 'deleted'].includes(result.result.status)) throw new TaskChangeError('invalid');
      this.notice = result.result.status === 'deleted' ? 'deleted' : 'updated'; this.noticeId++; this.publish();
      if (result.result.status === 'deleted') this.records.delete(taskKey(ref));
      await this.refreshNode(ref, true);
      return result.result;
    } catch (cause) {
      operation.pending = false;
      const uncertain = cause instanceof TaskChangeError ? cause.kind === 'uncertain' : isOutcomeUncertain(cause) || !acknowledged && !(cause instanceof RpcFailure) && !(cause instanceof RequestNotDispatched);
      if (!uncertain && this.changes.get(key) === operation) this.changes.delete(key);
      this.notice = 'failed'; this.noticeId++; this.publish();
      if (cause instanceof TaskChangeError) throw cause;
      throw new TaskChangeError(uncertain ? 'uncertain' : 'failed', String(cause));
    }
  }
  async recovery(ref: TaskRef, start_id: string, after = '0'): Promise<ScheduledWorkflowRecovery | undefined> {
    const current = this.capture();
    const result = await this.read(() => this.client.request({ method: 'workflow/scheduled/getRecovery', params: { session_id: ref.session_id, node_id: ref.node_id, start_id, after_effect_ordinal: after, limit: 20 } }, 'scheduled_workflow_recovery', undefined, current), current);
    return current() ? result.recovery ?? undefined : undefined;
  }
  async searchContent(query: string, interested: () => boolean): Promise<Set<string>> {
    const captured = this.capture(), current = () => captured() && interested();
    const matches = new Set<string>();
    let after: ScheduleCatalogCursor | null | undefined;
    do {
      if (!current()) break;
      const previous = after;
      const response = await this.read(() => this.client.request({ method: 'schedule/catalog', params: { after, limit: 100, query } }, 'schedule_catalog', undefined, current), current);
      if (!current()) break;
      if (response.page.unavailable.length) throw Error('Task search is incomplete');
      for (const { schedule } of response.page.entries) matches.add(taskKey(taskRef(schedule)));
      after = response.page.next;
      if (after && previous && JSON.stringify(after) === JSON.stringify(previous)) throw Error('Invalid task search page');
    } while (after && current());
    return matches;
  }
  async manageRun(ref: TaskRef, start_id: string, action: 'cancel' | 'reconcile'): Promise<ScheduledWorkflowRecovery | undefined> {
    this.mutations++; this.ensureFollowing();
    try { return await this.controlRun(ref, start_id, action); }
    finally { this.mutations--; this.releaseFollowing(); }
  }
  private async controlRun(ref: TaskRef, start_id: string, action: 'cancel' | 'reconcile'): Promise<ScheduledWorkflowRecovery | undefined> {
    const current = this.capture();
    const params = { session_id: ref.session_id, node_id: ref.node_id, start_id };
    if (action === 'cancel') await this.client.request({ method: 'workflow/scheduled/cancel', params }, 'scheduled_workflow_recovery', undefined, current);
    else await this.client.request({ method: 'workflow/scheduled/reconcile', params: { ...params, after_effect_ordinal: '0', limit: 20 } }, 'scheduled_workflow_recovery', undefined, current);
    return current() ? this.recovery(ref, start_id) : undefined;
  }
  dismissNotice = () => { this.notice = undefined; this.publish(); };
}

const owners = new WeakMap<AppServerClient, SchedulerModel>();
export function schedulerModel(client: AppServerClient) {
  let model = owners.get(client);
  if (!model) {
    model = new SchedulerModel(client);
    owners.set(client, model);
    const owned = model;
    client.onDispose(() => owned.dispose());
  }
  return model;
}
export function useTaskCatalog(client: AppServerClient) {
  const model = useMemo(() => schedulerModel(client), [client]);
  const snapshot = useSyncExternalStore(model.subscribe, model.getSnapshot, model.getSnapshot);
  return { model, ...snapshot };
}

export function parseTaskKey(value: string): TaskRef | undefined {
  try {
    const parts: unknown = JSON.parse(value);
    if (!Array.isArray(parts) || parts.length !== 4 || !parts.every(part => typeof part === 'string' && part.length > 0 && part.length <= 512)) return;
    const [session_id, node_id, conversation_id, ordinal] = parts as string[];
    if (!/^(0|[1-9][0-9]*)$/.test(ordinal) || BigInt(ordinal) > 18_446_744_073_709_551_615n) return;
    return { session_id, node_id, id: { conversation_id, ordinal } };
  } catch { return; }
}
