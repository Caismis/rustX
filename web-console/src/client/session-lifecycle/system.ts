import { createActor } from 'xstate';
import { observationCurrent, controlCurrent, sessionLifecycleMachine, type Command, type LifecycleActor, type LifecycleContext, type LifecycleEvent } from './machine';
import type { AttachmentTarget, RuntimeClientSessionDeletionResult } from '../../../../protocol/app-server/v42';
import { emptyFacts, type Observation, type SessionLifecyclePort } from './port';

/** Registry and work bound only. All Session decisions belong to the machine. */
export class SessionLifecycles {
  private actors = new Map<string, LifecycleActor>();
  private retired = new Set<LifecycleActor>();
  private authority: object = Object.freeze({});
  private count = 0;
  private sequence = 0;
  constructor(private readonly options: {
    transport(): { generation: number; connected: boolean };
    port(id: string): SessionLifecyclePort;
    project(id: string, current: LifecycleContext, previous?: LifecycleContext): void;
  }) {}
  private readonly budget = {
    available: () => this.count < 64,
    take: () => { this.count++; },
    release: () => { this.count--; },
    identity: () => ++this.sequence,
  };
  private prune() {
    for (const [id, actor] of this.actors) {
      if (this.actors.size < 32) break;
      const c = actor.getSnapshot().context;
      if (!c.active && !c.queue.length && !c.unresolved && !c.facts.target && !c.facts.deleting && (c.facts.attachmentIntent === 'released' || c.facts.attachment === 'error')) {
        actor.stop(); this.actors.delete(id);
      }
    }
  }
  actor(id: string): LifecycleActor {
    const existing = this.actors.get(id); if (existing) return existing;
    this.prune();
    if (this.actors.size + this.retired.size >= 64) throw new Error('Session lifecycle capacity reached. Release settled Sessions before opening more.');
    const actor = createActor(sessionLifecycleMachine, { input: { id, authority: this.authority, ...this.options.transport(), port: this.options.port(id), budget: this.budget } });
    this.actors.set(id, actor);
    let previous: LifecycleContext | undefined;
    actor.subscribe(snapshot => {
      const c = snapshot.context;
      if (previous && this.actors.get(id) === actor && (c.facts !== previous?.facts || c.epoch !== previous?.epoch || c.deleted !== previous?.deleted || c.active?.token !== previous?.active?.token)) this.options.project(id, c, previous);
      previous = c;
      if (this.retired.has(actor) && !c.active) { actor.stop(); this.retired.delete(actor); }
      if (c.deleted && !c.active && this.actors.get(id) === actor) { actor.stop(); this.actors.delete(id); }
    });
    actor.start(); return actor;
  }
  get(id: string) { return this.actors.get(id)?.getSnapshot().context; }
  epoch(id: string) { return this.get(id)?.epoch; }
  controls(id: string, proof?: Observation) { const c = this.get(id); return !!c && controlCurrent(c, proof); }
  observes(id: string, proof?: Observation) { const c = this.get(id); return !!c && observationCurrent(c, proof); }
  /** Capture once. Neither retained targets nor later intent can recreate this proof. */
  observe(id: string) {
    const proof = this.get(id)?.facts.attachmentObservation;
    if (!proof || !this.observes(id, proof)) return;
    return { proof, target: proof.target, current: () => this.observes(id, proof) };
  }
  target(id: string): AttachmentTarget {
    const c = this.get(id);
    if (!c || !controlCurrent(c, c.facts.attachmentObservation) || !c.facts.target) throw new Error('Session is not authoritatively attached. Refresh or reconnect.');
    return c.facts.target;
  }
  event(id: string, event: LifecycleEvent) { this.actors.get(id)?.send(event); }
  command(id: string, command: Omit<Command, 'resolve' | 'reject'>, throwRejected = false, reconnect = false): Promise<RuntimeClientSessionDeletionResult | undefined> {
    const actor = this.actor(id);
    let resolve!: Command['resolve'], reject!: Command['reject'];
    let rejection: unknown;
    const work = new Promise<RuntimeClientSessionDeletionResult | undefined>((done, fail) => { resolve = done; reject = fail; });
    const type = ({ open: 'OPEN', release: 'RELEASE', switch: 'SWITCH_NODE', delete: 'DELETE', recover: 'RECOVER', inspect: 'INSPECT_DELETION' } as const)[command.kind];
    actor.send({ type: reconnect ? 'RECONNECT' : type, command: { ...command, work, resolve, reject: error => { rejection = error; reject(error); } } });
    if (rejection && !actor.getSnapshot().context.active && actor.getSnapshot().context.facts === emptyFacts) { actor.stop(); this.actors.delete(id); }
    if (throwRejected && rejection) { void work.catch(() => {}); throw rejection; }
    return work;
  }
  async reconnect() {
    for (const id of this.actors.keys()) await this.command(id, { kind: 'open', current: () => true }, false, true).catch(() => {});
  }
  waitForOpen(id: string) {
    const c = this.get(id);
    return [...(c?.queue ?? []), ...(c?.active ? [c.active] : [])].find(op => op.command.kind === 'open')?.command.work;
  }
  transport(generation: number, connected: boolean) {
    for (const actor of this.actors.values()) actor.send({ type: 'TRANSPORT', generation, connected });
  }
  restore(id: string) { this.actor(id).send({ type: 'RESTORE_INTENT' }); }
  /** Acknowledging disconnected browser diagnostics is not native settlement. */
  forget(id: string) {
    const actor = this.actors.get(id); if (!actor) return;
    const c = actor.getSnapshot().context;
    if (c.connected || c.active || c.queue.length || c.facts.target) throw new Error('Lifecycle settlement is still outstanding.');
    actor.send({ type: 'AUTHORITY_REPLACED' }); actor.stop(); this.actors.delete(id);
  }
  replaceAuthority() {
    for (const actor of this.actors.values()) {
      this.retired.add(actor);
      actor.send({ type: 'AUTHORITY_REPLACED' });
      if (!actor.getSnapshot().context.active) { actor.stop(); this.retired.delete(actor); }
    }
    this.actors.clear();
    this.authority = Object.freeze({});
  }
  /** Bounded ownership inspection, used by deterministic tests. */
  diagnostics() { return { actors: this.actors.size, retired: this.retired.size, operations: this.count }; }
}
