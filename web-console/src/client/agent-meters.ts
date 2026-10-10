import type { AgentStatistics, AttachmentTarget, RuntimeClientAgent } from '../../../protocol/app-server/v42';

/** Native observations invalidate a read; they never supply usage evidence. */
export const meterDemand = (agent: RuntimeClientAgent) => JSON.stringify([
  agent.agent_id, agent.child_conversation_id, agent.activation_id,
  agent.state, agent.activation_state, agent.observation.revision,
]);
export interface MeterScope {
  target?: AttachmentTarget;
  current: () => boolean;
  inventory: () => readonly RuntimeClientAgent[];
}
export interface MeterReading { demand: string; metrics?: AgentStatistics; error?: string }
export interface MeterView { scope?: MeterScope; readings: ReadonlyMap<string, MeterReading>; blocked: boolean }
export type MeterOutcome = { settled: boolean; metrics?: AgentStatistics; error?: string; deferred?: never }
  | { settled: true; deferred: number; metrics?: never; error?: never };
type Read = (target: AttachmentTarget, id: string, current: () => boolean) => Promise<MeterOutcome>;
type Flight = { scope?: MeterScope; id: string; demand: string };
const EMPTY: MeterView = { readings: new Map(), blocked: false };

/** One client lifetime, one current inventory, no pending task closures.
 * Two current reads plus two retirement slots; a lost RPC acknowledgement
 * never releases physical-work accounting. No native cancellation is implied. */
export class AgentMeters {
  private scope?: MeterScope;
  private readings = new Map<string, MeterReading>();
  private flights = new Set<Flight>();
  private selected?: string;
  private cursor?: string;
  private priority = true;
  private admissionRevision = 0;
  private deferredAt?: number;
  private listeners = new Set<() => void>();
  private view: MeterView = EMPTY;
  constructor(private readonly read: Read) {}
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  getSnapshot = () => this.view;
  /** Request-pipeline evidence, never a render/revision-driven retry. */
  admissionAvailable(revision: number) {
    this.admissionRevision = revision;
    if (this.deferredAt !== undefined && revision > this.deferredAt) this.drain();
  }
  update(scope: MeterScope, selected?: string) {
    if (this.scope !== scope) {
      this.scope = scope; this.readings = new Map(); this.cursor = undefined; this.priority = true;
    }
    this.selected = selected;
    const inventory = scope.current() ? scope.inventory() : [];
    const demands = new Map(inventory.map(agent => [agent.agent_id, meterDemand(agent)]));
    for (const [id, reading] of this.readings) if (demands.get(id) !== reading.demand) this.readings.delete(id);
    this.drain();
  }
  retire(scope: MeterScope) {
    if (this.scope !== scope) return;
    this.scope = undefined; this.readings = new Map(); this.selected = this.cursor = undefined;
    this.publish();
  }
  private valid(scope: MeterScope, id: string, demand: string) {
    return this.scope === scope && scope.current() && scope.inventory().some(agent => agent.agent_id === id && meterDemand(agent) === demand);
  }
  private drain() {
    if (this.deferredAt !== undefined && this.admissionRevision > this.deferredAt) this.deferredAt = undefined;
    const scope = this.scope;
    if (scope?.target && scope.current() && this.deferredAt === undefined) {
      const agents = scope.inventory();
      while (this.flights.size < 4 && [...this.flights].filter(flight => flight.scope === scope).length < 2) {
        const eligible = (agent: RuntimeClientAgent) => this.readings.get(agent.agent_id)?.demand !== meterDemand(agent)
          && ![...this.flights].some(flight => flight.scope === scope && flight.id === agent.agent_id);
        const preferred = this.priority ? agents.find(agent => agent.agent_id === this.selected && eligible(agent)) : undefined;
        let next = preferred;
        if (!next) {
          const start = agents.findIndex(agent => agent.agent_id === this.cursor) + 1;
          for (let offset = 0; offset < agents.length; offset++) {
            const candidate = agents[(start + offset) % agents.length];
            if (eligible(candidate)) { next = candidate; break; }
          }
        }
        if (!next) break;
        // At most every other admission jumps the cursor for selection.
        this.priority = !preferred;
        if (!preferred) this.cursor = next.agent_id;
        const flight: Flight = { scope, id: next.agent_id, demand: meterDemand(next) };
        this.flights.add(flight);
        const current = () => this.valid(scope, flight.id, flight.demand);
        void this.read(scope.target, flight.id, current).then(outcome => {
          if (outcome.settled) this.flights.delete(flight);
          // A rejected observer without settlement remains a charged slot.
          else flight.scope = undefined;
          if (outcome.deferred !== undefined) {
            // Capacity is client-wide. Preserve the inventory demand but pause
            // admission until a strictly newer readiness cut, even across scopes.
            this.deferredAt = Math.max(this.deferredAt ?? -1, outcome.deferred);
          } else if (current()) this.readings.set(flight.id, { demand: flight.demand, metrics: outcome.metrics, error: outcome.error });
        }, error => {
          flight.scope = undefined;
          if (current()) this.readings.set(flight.id, { demand: flight.demand, error: String(error) });
        }).finally(() => this.drain());
      }
    }
    this.publish();
  }
  private publish() {
    this.view = { scope: this.scope, readings: new Map(this.readings), blocked: this.flights.size >= 4 };
    for (const listener of this.listeners) listener();
  }
}
