import { expect, it } from 'vitest';
import { AgentMeters, meterDemand, type MeterOutcome, type MeterScope } from '../src/client/agent-meters';
import type { RuntimeClientAgent } from '../../protocol/app-server/v38';
import { agentMetrics } from './agent-statistics-fixture';
import { cfg3Target } from './cfg3-fixture';
const agent = (id: number, revision = '1') => ({ agent_id: String(id), activation_id: 'activation', child_conversation_id: `child-${id}`, state: 'inactive', activation_state: 'succeeded', observation: { revision } }) as RuntimeClientAgent;
function fixture() {
  let active = 0, maximum = 0;
  const requests: { id: string; current: () => boolean; release: (outcome: MeterOutcome) => void }[] = [];
  const meters = new AgentMeters((_target, id, current) => new Promise(resolve => {
    maximum = Math.max(maximum, ++active);
    requests.push({ id, current, release: outcome => { if (outcome.settled) active--; resolve(outcome); } });
  }));
  const scope = (agents: RuntimeClientAgent[]): MeterScope => ({ target: cfg3Target, current: () => true, inventory: () => agents });
  const settle = async (index: number, outcome: MeterOutcome = { settled: true, metrics: agentMetrics }) => {
    const changed = new Promise<void>(resolve => { const retire = meters.subscribe(() => { retire(); resolve(); }); });
    requests[index].release(outcome); await changed;
  };
  return { meters, requests, scope, settle, get maximum() { return maximum; }, get active() { return active; } };
}
it('services every stable demand in an 80-Agent inventory with only two outstanding reads', async () => {
  const f = fixture(), agents = Array.from({ length: 80 }, (_, i) => agent(i));
  f.meters.update(f.scope(agents)); expect(f.requests).toHaveLength(2);
  for (let i = 0; i < 80; i++) await f.settle(i);
  expect(f.requests.map(row => row.id)).toEqual(agents.map(row => row.agent_id));
  expect(f.maximum).toBe(2); expect(f.active).toBe(0);
  expect(f.meters.getSnapshot().readings.size).toBe(80);
});
it('coalesces 10,000 revisions and fairly services distinct Agents before revisiting a busy one', async () => {
  const f = fixture(), agents = Array.from({ length: 80 }, (_, i) => agent(i)), scope = f.scope(agents);
  f.meters.update(scope);
  for (let i = 0; i < 10000; i++) { agents[0] = agent(0, String(i + 2)); f.meters.update(scope); }
  expect(f.requests).toHaveLength(2); expect(f.requests[0].current()).toBe(false);
  for (let i = 0; i < 80; i++) await f.settle(i);
  expect(f.requests.map(row => row.id)).toEqual([...agents.map(row => row.agent_id), '0']);
  await f.settle(80);
  expect(f.meters.getSnapshot().readings.get('0')!.demand).toBe(meterDemand(agents[0]));
  f.meters.update(scope); expect(f.requests).toHaveLength(81); expect(f.maximum).toBe(2);
});
it('prioritizes an unread selected Agent beyond the old queue capacity, then admits ordinary demand', async () => {
  const f = fixture(), agents = Array.from({ length: 80 }, (_, i) => agent(i)), scope = f.scope(agents);
  f.meters.update(scope); f.meters.update(scope, '79'); await f.settle(0);
  expect(f.requests[2].id).toBe('79');
  agents[79] = agent(79, '2'); f.meters.update(scope, '79'); await f.settle(2);
  expect(f.requests[3].id).toBe('2');
  await f.settle(1); expect(f.requests[4].id).toBe('79');
  expect(f.maximum).toBe(2);
});
it('reserves successor capacity while accounting for all retired reads across repeated switches', async () => {
  const f = fixture(), a = f.scope([agent(0), agent(1), agent(2)]), b = f.scope([agent(10), agent(11), agent(12)]);
  f.meters.update(a); f.meters.retire(a); f.meters.update(b);
  expect(f.requests.map(row => row.id)).toEqual(['0', '1', '10', '11']);
  expect(f.requests[0].current()).toBe(false);
  for (let i = 0; i < 100; i++) { f.meters.update(f.scope([agent(i)])); }
  expect(f.requests).toHaveLength(4); expect(f.active).toBe(4);
  expect(f.meters.getSnapshot().blocked).toBe(true);
  await f.settle(0); expect(f.requests[4].id).toBe('99');
  expect(f.meters.getSnapshot().readings.size).toBe(0);
  await f.settle(4); expect(f.meters.getSnapshot().readings.has('99')).toBe(true);
  await f.settle(1); await f.settle(2); await f.settle(3);
  expect(f.requests).toHaveLength(5); expect(f.maximum).toBe(4); expect(f.active).toBe(0);
});
it('retires disappeared Agents and fences activation and authority before publishing or dispatching', async () => {
  const f = fixture(), agents = [agent(0), agent(1), agent(2)], scope = f.scope(agents);
  f.meters.update(scope); agents.pop(); agents[0] = { ...agent(0), activation_id: 'successor' }; f.meters.update(scope);
  expect(f.requests[0].current()).toBe(false); await f.settle(0);
  expect(f.meters.getSnapshot().readings.has('0')).toBe(false);
  expect(f.requests.map(row => row.id)).toEqual(['0', '1', '0']);
  scope.current = () => false; await f.settle(1); await f.settle(2);
  expect(f.meters.getSnapshot().readings.size).toBe(0); expect(f.requests).toHaveLength(3);
});
it('lost acknowledgements stay charged and definitive errors remain terminal for unchanged demand', async () => {
  const f = fixture(), scope = f.scope([agent(0), agent(1)]);
  f.meters.update(scope); await f.settle(0, { settled: true, error: 'native refusal' });
  expect(f.meters.getSnapshot().readings.get('0')?.error).toBe('native refusal');
  f.meters.update(scope); expect(f.requests).toHaveLength(2);
  await f.settle(1, { settled: false, error: 'transport lost' });
  expect(f.active).toBe(1);
  for (let i = 0; i < 3; i++) {
    f.meters.update(f.scope([agent(10 + i)])); await f.settle(2 + i, { settled: false, error: 'transport lost' });
  }
  f.meters.update(f.scope([agent(99)]));
  expect(f.requests).toHaveLength(5); expect(f.active).toBe(4); expect(f.maximum).toBe(4);
  expect(f.meters.getSnapshot().blocked).toBe(true);
});
it('continuous selected-Agent updates cannot starve the rest of the inventory', async () => {
  const f = fixture(), agents = Array.from({ length: 10 }, (_, i) => agent(i)), scope = f.scope(agents);
  f.meters.update(scope, '0');
  for (let i = 0; i < 20; i++) {
    agents[0] = agent(0, String(i + 2)); f.meters.update(scope, '0');
    await f.settle(i);
  }
  for (let id = 1; id < 10; id++) expect(f.meters.getSnapshot().readings.has(String(id))).toBe(true);
  expect(f.maximum).toBe(2);
});
it('capacity deferral preserves all 80 demands and waits for a strictly newer readiness cut', async () => {
  const f = fixture(), agents = Array.from({ length: 80 }, (_, i) => agent(i)), scope = f.scope(agents);
  f.meters.admissionAvailable(7); f.meters.update(scope);
  await f.settle(0, { settled: true, deferred: 7 });
  await f.settle(1, { settled: true, deferred: 7 });
  expect(f.active).toBe(0); expect(f.meters.getSnapshot().readings.size).toBe(0);
  for (let i = 0; i < 100; i++) { f.meters.update(scope, '79'); f.meters.admissionAvailable(7); }
  expect(f.requests).toHaveLength(2);
  f.meters.admissionAvailable(8); expect(f.requests).toHaveLength(4);
  expect(f.requests[2].id).toBe('79');
  for (let i = 2; i < 82; i++) await f.settle(i);
  expect(new Set(f.requests.slice(2).map(row => row.id)).size).toBe(80);
  expect(f.requests).toHaveLength(82); expect(f.maximum).toBe(2); expect(f.active).toBe(0);
  expect(f.meters.getSnapshot().readings.size).toBe(80);
});
it('readiness arriving before the refusal continuation is not lost', async () => {
  const f = fixture(), scope = f.scope([agent(0)]);
  f.meters.update(scope); f.meters.admissionAvailable(1);
  await f.settle(0, { settled: true, deferred: 0 });
  expect(f.requests).toHaveLength(2);
  await f.settle(1); expect(f.meters.getSnapshot().readings.size).toBe(1);
  f.meters.admissionAvailable(2); f.meters.update(scope); expect(f.requests).toHaveLength(2);
});
it('synchronous authority revocation fences settlement-driven admission before an observer update', async () => {
  const f = fixture(), agents = [agent(0), agent(1), agent(2)];
  let revision = 0;
  const scope = { ...f.scope(agents), current: () => revision === 0 };
  f.meters.update(scope); revision++;
  await f.settle(0); await f.settle(1);
  expect(f.requests).toHaveLength(2); expect(f.meters.getSnapshot().readings.size).toBe(0);
  f.meters.update(f.scope(agents));
  for (let i = 2; i < 5; i++) await f.settle(i);
  expect(f.meters.getSnapshot().readings.size).toBe(3); expect(f.maximum).toBe(2);
});
