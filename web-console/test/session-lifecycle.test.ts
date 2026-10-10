import { expect, it } from 'vitest';
import type { AttachmentTarget, RuntimeClientSessionDeletionResult } from '../../protocol/app-server/v41';
import { SessionLifecycles } from '../src/client/session-lifecycle/system';
import { controlCurrent, type LifecycleContext } from '../src/client/session-lifecycle/machine';
import type { Admission, Attached, FailureKind, SessionLifecyclePort } from '../src/client/session-lifecycle/port';
import { snapshot } from './fixture';

function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (error: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  void promise.catch(() => {}); return { promise, resolve, reject };
}
class Failure extends Error { constructor(readonly kind: FailureKind) { super(kind); } }
interface Call { generation: number; kind: string; node?: string; target?: AttachmentTarget; result: ReturnType<typeof deferred<unknown>> }
/** The machine is real. Only finite native/Host I/O is scripted; every order is
 * established by a call barrier, explicit ACK or deferred admission. */
class Harness {
  generation = 1; connected = true; selected = 'A'; resident?: string;
  claims = new Set<AttachmentTarget>(); calls: Call[] = []; sequence = 0;
  projections: LifecycleContext[] = [];
  host?: ReturnType<typeof deferred<void>>; hostEntered = deferred<void>();
  validation?: ReturnType<typeof deferred<void>>; validationEntered = deferred<void>();
  auto = false;
  beforeDispatch?: () => void;
  private listeners = new Set<() => void>();
  readonly port: SessionLifecyclePort = {
    resolveNode: async () => ({ node: this.selected, conversation: `conversation-${this.selected}` }),
    conversation: async node => `conversation-${node}`,
    admit: async current => {
      this.hostEntered.resolve(); if (this.host) await this.host.promise;
      return { current, validate: async () => true };
    },
    attach: (node, admission) => this.rpc<Attached>('attach', admission, { node }),
    detach: (target, admission) => this.rpc<void>('detach', admission, { target }),
    switchNode: (target, node, admission) => this.rpc('switch', admission, { target, node }),
    delete: (_revision, admission) => this.rpc('delete', admission),
    recover: admission => this.rpc('recover', admission),
    inspectDeletion: async () => ({ status: 'preview', preview: { session_id: 'S', target_revision: this.selected, owned_node_count: 2, owned_conversation_count: 2, owned_child_count: 0 } }),
    observeAttached: async () => {}, refresh: async () => {}, cold: () => {},
    classify: error => error instanceof Failure ? error.kind : 'local',
  };
  readonly system = new SessionLifecycles({
    transport: () => ({ generation: this.generation, connected: this.connected }),
    port: () => this.port,
    project: (_id, context) => { this.projections.push(context); },
  });
  private async rpc<T>(kind: string, admission: Admission, input: Partial<Call> = {}): Promise<T> {
    this.validationEntered.resolve(); if (this.validation) await this.validation.promise;
    if (!admission.current() || !await admission.validate(new AbortController().signal)) throw new Failure('unsent');
    this.beforeDispatch?.();
    if (!admission.current()) throw new Failure('unsent');
    admission.sent?.();
    const call: Call = { kind, generation: this.generation, result: deferred<unknown>(), ...input }; this.calls.push(call);
    for (const notify of this.listeners) notify();
    if (this.auto) this.ack(call);
    return call.result.promise as Promise<T>;
  }
  next(kind: string, count = 1): Promise<Call> {
    const find = () => this.calls.filter(call => call.kind === kind)[count - 1];
    const found = find(); if (found) return Promise.resolve(found);
    return new Promise(resolve => { const check = () => { const value = find(); if (value) { this.listeners.delete(check); resolve(value); } }; this.listeners.add(check); });
  }
  private dropClaim(target: AttachmentTarget) {
    for (const claim of this.claims) if (claim.attachment_id === target.attachment_id && claim.runtime_incarnation === target.runtime_incarnation && claim.session_id === target.session_id && claim.conversation_id === target.conversation_id) this.claims.delete(claim);
  }
  ack(call: Call) {
    if (call.kind === 'attach') {
      const conversation = `conversation-${call.node}`;
      if (this.resident && this.resident !== conversation) { call.result.reject(new Failure('refused')); return; }
      expect(this.claims.size).toBe(0); this.resident = conversation;
      const target = { session_id: 'S', conversation_id: conversation, runtime_incarnation: '1', attachment_id: String(++this.sequence) };
      if (call.generation === this.generation && this.connected) this.claims.add(target);
      call.result.resolve({ type: 'attached', target, snapshot: { ...snapshot(), conversation_id: conversation }, cursor: '0' });
    } else if (call.kind === 'switch') {
      this.dropClaim(call.target!); this.selected = call.node!; this.resident = `conversation-${call.node}`;
      call.result.resolve({ id: 'S', active_node: call.node, active_conversation_id: this.resident });
    } else if (call.kind === 'detach') { this.dropClaim(call.target!); call.result.resolve(undefined); }
    else call.result.resolve({ status: 'stale', session_id: 'S' });
  }
  command(kind: 'open' | 'release' | 'switch' | 'delete' | 'recover' | 'inspect', node?: string) {
    return this.system.command('S', { kind, node, revision: this.selected, current: () => true });
  }
  get context() { return this.system.get('S')!; }
  get facts() { return this.context.facts; }
  async attached() { const work = this.command('open', 'A'); this.ack(await this.next('attach')); await work; return this.facts.attachmentObservation!; }
  loss() { this.connected = false; this.generation++; this.claims.clear(); this.system.transport(this.generation, false); }
  restored() { this.connected = true; this.system.transport(this.generation, true); }
}

it('Release during Host admission settles unsent absence without claims or native requests', async () => {
  const h = new Harness(); h.host = deferred<void>();
  const open = h.command('open', 'A'); await h.hostEntered.promise;
  const release = h.command('release'); expect(h.facts.attachmentIntent).toBe('released');
  h.host.resolve(); await Promise.all([open, release]);
  expect(h.calls).toEqual([]); expect(h.claims.size).toBe(0);
  expect(h.facts).toMatchObject({ attachment: 'detached', attachmentIntent: 'released' });
  expect(h.system.diagnostics().operations).toBe(0);
});

it('transmitted obsolete Attach retains one cleanup claim until exact Detach acknowledgement', async () => {
  const h = new Harness(), open = h.command('open', 'A'), attach = await h.next('attach');
  const release = h.command('release'); h.ack(attach); await open;
  const detach = await h.next('detach');
  expect(h.facts.attachmentObservation).toBeUndefined(); expect(h.claims.size).toBe(1);
  expect(detach.target).toBe(h.facts.target);
  h.ack(detach); await release;
  expect(h.calls.map(c => c.kind)).toEqual(['attach', 'detach']); expect(h.claims.size).toBe(0);
});

it('equivalent Opens share one native claim; conflicting Open cannot change admitted identity', async () => {
  const h = new Harness(), first = h.command('open', 'A'), attach = await h.next('attach');
  const repeated = Array.from({ length: 5 }, () => h.command('open', 'A'));
  await expect(h.command('open', 'B')).rejects.toThrow('branch switching');
  expect(h.facts.nodeId).toBe('A'); h.ack(attach); await Promise.all([first, ...repeated]);
  expect(h.calls).toHaveLength(1); expect(h.claims.size).toBe(1);
  expect(controlCurrent(h.context, h.facts.attachmentObservation)).toBe(true);
});

it.each(['A', 'B'])('Release then Open %s waits for exact cleanup and obeys resident Conversation', async node => {
  const h = new Harness(), old = await h.attached();
  const release = h.command('release'), open = h.command('open', node);
  const outcome = open.then(() => 'attached', error => String(error));
  expect(controlCurrent(h.context, old)).toBe(false);
  h.ack(await h.next('detach')); await release;
  h.ack(await h.next('attach', 2));
  expect(await outcome).toContain(node === 'A' ? 'attached' : 'refused');
  expect(h.claims.size).toBe(node === 'A' ? 1 : 0);
  expect(h.resident).toBe('conversation-A'); expect(controlCurrent(h.context, old)).toBe(false);
});

it('failed Detach retains cleanup ownership and cannot authorize a queued Open', async () => {
  const h = new Harness(); await h.attached(); const old = h.facts.target;
  const release = h.command('release'), rejected = expect(release).rejects.toThrow('refused');
  const open = h.command('open', 'A'), refused = expect(open).rejects.toThrow('Release the retained attachment');
  (await h.next('detach')).result.reject(new Failure('refused')); await Promise.all([rejected, refused]);
  expect(h.facts.target).toBe(old); expect(h.claims.size).toBe(1); expect(h.calls.map(c => c.kind)).toEqual(['attach', 'detach']);
});

it.each(['normal', 'release', 'closed', 'release-and-closed'])('Switch native commit survives intent and Route changes: %s', async phase => {
  const h = new Harness(), old = await h.attached();
  const work = h.command('switch', 'B'), call = await h.next('switch');
  const release = phase.includes('release') ? h.command('release') : undefined;
  if (phase.includes('closed')) h.system.event('S', { type: 'ROUTE_CLOSED', target: old.target });
  const before = h.facts;
  await expect(h.command('delete')).rejects.toThrow('Node switch');
  await expect(h.command('open', 'A')).rejects.toThrow('Node switch');
  expect(h.facts).toBe(before); expect(h.calls.map(c => c.kind)).toEqual(['attach', 'switch']);
  h.ack(call); await Promise.all([work, release]);
  expect(h.facts).toMatchObject({ nodeId: 'B', nodeConversationId: 'conversation-B', attachment: 'detached', attachmentIntent: release ? 'released' : 'wanted' });
  expect(h.context.selection).toEqual({ node: 'B', conversation: 'conversation-B' });
  expect(h.resident).toBe('conversation-B'); expect(h.claims.size).toBe(0); expect(controlCurrent(h.context, old)).toBe(false);
});

it('Switch pre-send revocation cannot enter native dispatch and cannot lose its Delete exclusion', async () => {
  const h = new Harness(); await h.attached(); h.validation = deferred<void>(); h.validationEntered = deferred<void>();
  const work = h.command('switch', 'B'), refused = expect(work).rejects.toThrow('unsent');
  await h.validationEntered.promise;
  const release = h.command('release'); await expect(h.command('delete')).rejects.toThrow('Node switch');
  h.validation.resolve(); await refused;
  h.ack(await h.next('detach')); await release;
  expect(h.calls.map(c => c.kind)).toEqual(['attach', 'detach']); expect(h.facts.nodeId).toBe('A'); expect(h.claims.size).toBe(0);
});

it.each(['before-transition', 'after-unload'])('Switch failure is fail closed with retained cleanup evidence: %s', async phase => {
  const h = new Harness(), old = await h.attached();
  const work = h.command('switch', 'B'), refused = expect(work).rejects.toThrow('refused'), call = await h.next('switch');
  await expect(h.command('delete')).rejects.toThrow('Node switch');
  if (phase === 'after-unload') { h.resident = undefined; h.selected = 'B'; h.claims.clear(); }
  call.result.reject(new Failure('refused')); await refused;
  expect(h.facts.nodeId).toBeUndefined(); expect(h.facts.target).toBe(old.target); expect(controlCurrent(h.context, old)).toBe(false);
  const release = h.command('release'); h.ack(await h.next('detach')); await release;
  const open = h.command('open'); const attach = await h.next('attach', 2);
  expect(attach.node).toBe(phase === 'after-unload' ? 'B' : 'A'); h.ack(attach); await open; expect(h.claims.size).toBe(1);
});

it.each(['attach', 'switch', 'delete'] as const)('unknown transmitted %s settles once without automatic replay after reconnect', async kind => {
  const h = new Harness(); if (kind !== 'attach') await h.attached();
  const work = h.command(kind === 'attach' ? 'open' : kind, kind === 'switch' ? 'B' : 'A');
  const rejected = expect(work).rejects.toThrow('uncertain'), call = await h.next(kind);
  h.loss(); call.result.reject(new Failure('uncertain')); await rejected; h.restored();
  expect(h.context.unresolved?.kind).toBe(kind === 'attach' ? 'open' : kind);
  expect(h.calls.filter(c => c.kind === kind)).toHaveLength(1); expect(h.facts.attachmentObservation).toBeUndefined();
  expect(h.system.diagnostics().operations).toBe(0);
  if (kind === 'switch') expect(h.facts.nodeId).toBeUndefined();
  if (kind === 'delete') { expect(h.facts.deleting).toBe(true); await h.command('inspect'); expect(h.facts.deleting).toBe(false); }
});

it('Delete admitted first excludes Switch; a rejected Delete never restores control', async () => {
  const h = new Harness(), old = await h.attached();
  const work = h.command('delete'), call = await h.next('delete');
  expect(controlCurrent(h.context, old)).toBe(false); await expect(h.command('switch', 'B')).rejects.toThrow();
  call.result.resolve({ status: 'stale', session_id: 'S' } satisfies RuntimeClientSessionDeletionResult); await work;
  expect(h.facts.deleting).toBe(false); expect(controlCurrent(h.context, old)).toBe(false);
  expect(h.calls.map(c => c.kind)).toEqual(['attach', 'delete']);
});

it('obsolete generation acknowledgement cannot publish a claim into a restored generation', async () => {
  const h = new Harness(), work = h.command('open', 'A'), call = await h.next('attach');
  h.loss(); h.restored(); h.ack(call); await work;
  expect(h.facts.target).toBeUndefined(); expect(h.facts.attachmentObservation).toBeUndefined();
  expect(h.calls).toHaveLength(1); expect(h.claims.size).toBe(0); expect(h.system.diagnostics().operations).toBe(0);
});

it.each(['switch', 'delete'] as const)('authority retirement retains only outstanding %s settlement then releases the actor', async kind => {
  const h = new Harness(); await h.attached();
  const work = h.command(kind, 'B'), rejected = expect(work).rejects.toThrow('uncertain'), call = await h.next(kind);
  h.loss(); h.system.replaceAuthority();
  expect(h.system.diagnostics()).toMatchObject({ actors: 0, retired: 1 });
  call.result.reject(new Failure('uncertain')); await rejected;
  expect(h.system.diagnostics()).toEqual({ actors: 0, retired: 0, operations: 0 });
});

it('inactive actors remain bounded and confirmed deletion retires the actor', async () => {
  const h = new Harness();
  for (let index = 0; index < 100; index++) await h.system.command(String(index), { kind: 'release', current: () => true });
  expect(h.system.diagnostics().actors).toBeLessThanOrEqual(32); expect(h.system.diagnostics().operations).toBe(0);
  const deleting = h.command('delete'), call = await h.next('delete');
  call.result.resolve({ status: 'deleted', session_id: 'S' }); await deleting;
  expect(h.system.get('S')).toBeUndefined(); expect(h.system.diagnostics().operations).toBe(0);
});

it('bounded operation admission never prevents synchronous Release revocation', async () => {
  const h = new Harness(), opening = h.command('open', 'A'), call = await h.next('attach');
  const queued = Array.from({ length: 63 }, () => h.command('open', 'A'));
  expect(h.system.diagnostics().operations).toBe(64);
  await expect(h.command('release')).rejects.toThrow('capacity');
  expect(h.facts.attachmentIntent).toBe('released'); expect(h.facts.attachmentObservation).toBeUndefined();
  h.ack(call); await Promise.all([opening, ...queued]);
  expect(h.calls).toHaveLength(1); expect(h.claims.size).toBe(1); expect(h.system.diagnostics().operations).toBe(0);
  const release = h.command('release'); h.ack(await h.next('detach')); await release;
  expect(h.claims.size).toBe(0); expect(h.system.diagnostics().operations).toBe(0);
});

it('control and target identities are immutable projections, never writable admission state', async () => {
  const h = new Harness(), proof = await h.attached();
  expect(Reflect.set(proof, 'intentRevision', 99)).toBe(false);
  expect(Reflect.set(proof.target, 'attachment_id', 'forged')).toBe(false);
  expect(controlCurrent(h.context, proof)).toBe(true);
  const release = h.command('release'); expect(controlCurrent(h.context, proof)).toBe(false);
  h.ack(await h.next('detach')); await release;
  const opening = h.command('open', 'A'); h.ack(await h.next('attach', 2)); await opening;
  expect(controlCurrent(h.context, proof)).toBe(false); expect(h.facts.attachmentObservation).not.toBe(proof);
});

it('a known-unsent Host wait retires on authority replacement without retaining an actor', async () => {
  const h = new Harness(); h.host = deferred<void>();
  const open = h.command('open', 'A'); await h.hostEntered.promise;
  h.loss(); h.system.replaceAuthority(); await open;
  expect(h.system.diagnostics()).toEqual({ actors: 0, retired: 0, operations: 0 });
  h.host.resolve();
  expect(h.calls).toEqual([]);
});


it.each(['preparing', 'validation'] as const)('proven unsent Delete clears its pending state without reopening: %s', async phase => {
  const h = new Harness(), proof = await h.attached();
  h.validation = deferred<void>(); h.validationEntered = deferred<void>();
  const deleting = h.command('delete');
  const outcome = deleting.catch(error => error);
  if (phase === 'validation') await h.validationEntered.promise;
  expect(controlCurrent(h.context, proof)).toBe(false);
  expect(h.facts.attachmentIntent).toBe('released');
  h.loss(); h.validation.resolve(); await outcome;
  expect(h.facts.deleting).toBe(false); expect(h.context.unresolved).toBeUndefined();
  expect(h.system.diagnostics().operations).toBe(0);
  h.restored(); await h.system.reconnect();
  expect(h.calls.map(call => call.kind)).toEqual(['attach']); expect(h.claims.size).toBe(0);
  await h.command('release'); expect(h.facts.attachment).toBe('detached');
});

it('disconnected Delete is refused before projection or native work', async () => {
  const h = new Harness(); h.loss(); const count = h.projections.length;
  await expect(h.command('delete')).rejects.toThrow('Connect before');
  expect(h.projections).toHaveLength(count); expect(h.calls).toEqual([]);
  expect(h.system.diagnostics()).toEqual({ actors: 0, retired: 0, operations: 0 });
});

it('observation status carries exact actor proof; retained cleanup identity cannot restore it', async () => {
  const h = new Harness(), proof = await h.attached();
  h.system.event('S', { type: 'OBSERVATION', proof, status: 'resynchronizing' });
  expect(h.system.observes('S', proof)).toBe(true); expect(h.system.controls('S', proof)).toBe(false);
  expect(h.system.observe('S')?.proof).toBe(proof);
  h.system.event('S', { type: 'OBSERVATION', proof: { ...proof }, status: 'attached' });
  expect(h.facts.attachment).toBe('resynchronizing');
  h.system.event('S', { type: 'OBSERVATION', proof, status: 'attached' });
  expect(h.system.controls('S', proof)).toBe(true);
  const release = h.command('release'), detach = await h.next('detach');
  const facts = h.facts;
  h.system.event('S', { type: 'OBSERVATION', proof, status: 'attached' });
  expect(h.facts).toBe(facts); expect(h.facts.target).toBe(proof.target);
  expect(h.system.observe('S')).toBeUndefined(); expect(h.system.controls('S', proof)).toBe(false);
  h.ack(detach); await release; expect(h.claims.size).toBe(0);
  const opening = h.command('open', 'A'); h.ack(await h.next('attach', 2)); await opening;
  const fresh = h.facts.attachmentObservation!; expect(fresh).not.toBe(proof);
  h.system.event('S', { type: 'OBSERVATION', proof, status: 'stale' });
  expect(h.system.observe('S')?.proof).toBe(fresh); expect(h.system.controls('S', fresh)).toBe(true);
  expect(h.calls.map(call => call.kind)).toEqual(['attach', 'detach', 'attach']);
});

it('Open navigation releasing synchronously at the final predicate cannot dispatch the old token', async () => {
  const h = new Harness(); h.auto = true; h.validation = deferred<void>();
  let final = false, release: Promise<unknown> | undefined; const order: string[] = [];
  h.beforeDispatch = () => { final = true; };
  const open = h.system.command('S', { kind: 'open', node: 'A', current: () => {
    if (final && !release) { order.push('navigation'); release = h.command('release'); order.push('released'); }
    return true;
  } }); const outcome = open.catch(error => error);
  await h.validationEntered.promise; const op = h.context.active!; h.validation.resolve();
  await outcome; await release;
  expect(order).toEqual(['navigation', 'released']); expect(h.facts.attachmentIntentRevision).toBeGreaterThan(op.revision);
  expect(h.calls.map(c => c.kind)).toEqual([]); expect(h.claims.size).toBe(0);
  expect(h.facts).toMatchObject({ attachmentIntent: 'released', attachment: 'detached' }); expect(h.context.unresolved).toBeUndefined();
  expect(h.system.diagnostics().operations).toBe(0);
});


it('a Host proof cannot restore Open authority revoked inside its own current callback', async () => {
  const h = new Harness(); let release: Promise<unknown> | undefined;
  h.port.admit = async () => ({ current: () => { release ??= h.command('release'); return true; }, validate: async () => true });
  await h.command('open', 'A'); await release;
  expect(h.calls).toEqual([]); expect(h.claims.size).toBe(0);
  expect(h.facts).toMatchObject({ attachmentIntent: 'released', attachment: 'detached' });
  expect(h.system.diagnostics().operations).toBe(0);
});

it('ACK freshness revocation retains cleanup without calling attached or observing its Snapshot', async () => {
  const h = new Harness(); let acknowledged = false, release: Promise<unknown> | undefined, published = 0;
  h.port.observeAttached = async () => { published++; };
  const open = h.system.command('S', { kind: 'open', node: 'A', current: () => {
    if (acknowledged) release ??= h.command('release');
    return true;
  }, attached: () => { published++; } });
  const attach = await h.next('attach'); acknowledged = true; h.ack(attach); await open;
  const detach = await h.next('detach');
  expect(published).toBe(0); expect(h.claims.size).toBe(1); expect(h.facts.attachmentObservation).toBeUndefined();
  h.ack(detach); await release;
  expect(h.calls.map(c => c.kind)).toEqual(['attach', 'detach']); expect(h.claims.size).toBe(0);
});

it('obsolete navigation failure retires outside actor actions without publishing a current error', async () => {
  const h = new Harness(); let fresh = true;
  h.port.admit = async () => { fresh = false; throw new Failure('unsent'); };
  await expect(h.system.command('S', { kind: 'open', node: 'A', current: () => fresh })).rejects.toThrow('unsent');
  expect(h.calls).toEqual([]); expect(h.facts.attachment).toBe('detached'); expect(h.facts.error).toBeUndefined();
  expect(h.system.diagnostics().operations).toBe(0);
});
