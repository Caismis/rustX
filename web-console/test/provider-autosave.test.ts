import { afterEach, expect, it, vi } from 'vitest';
import { createActor, SimulatedClock } from 'xstate';
import type { ProviderWrite, SourceMutation } from '../../protocol/app-server/v77';
import { settingsTargetMachine } from '../src/app/settings/machines/settings-target';
import type { ConfigurationPort, WriteOutcome } from '../src/app/settings/machines/port';
import { revisionSelector, userSettingsTarget } from '../src/app/settings/projection';
import { cfg3Source } from './cfg3-data';

const flush = async () => { for (let turn = 0; turn < 16; turn++) await Promise.resolve(); };
const stops: (() => void)[] = [];
afterEach(() => stops.splice(0).forEach(stop => stop()));

async function fixture() {
  const source = cfg3Source();
  const clock = new SimulatedClock();
  const write = vi.fn<ConfigurationPort['write']>(async () => ({ acknowledgement: { revision: source.user.revision } }));
  const actor = createActor(settingsTargetMachine, { clock, input: {
    target: userSettingsTarget, connection: 'connected', generation: 1, publication: undefined,
    port: { ownsReread: false, publication: () => undefined,
      read: async () => structuredClone(source), write, reconcile: async () => {} },
  } }).start();
  stops.push(() => actor.stop());
  actor.send({ type: 'ATTACH' });
  await flush();
  const mutation: SourceMutation = { kind: 'config', mutation: { unit: 'provider', id: 'transport', authored: null } };
  const identity = JSON.stringify(mutation);
  const selector = revisionSelector(mutation);
  const edit = (url: string, action: 'idle' | 'blur' | 'now' = 'idle') => {
    const value: ProviderWrite = { protocol: 'openai_responses', base_url: url, credential: { kind: 'retain' } };
    actor.send({ type: 'UNIT.EDIT', identity, selector, revision: source.user.revision, value });
    actor.send({ type: 'UNIT.AUTOSAVE', identity, action });
    const snapshot = actor.getSnapshot().context.units[identity].getSnapshot();
    expect(snapshot.matches({ intent: 'dirty' })).toBe(true);
    expect(snapshot.matches({ base: 'pinned' })).toBe(true);
  };
  const gesture = (action: 'flush' | 'hold' | 'cancel') => actor.send({ type: 'UNIT.AUTOSAVE', identity, action });
  return { source, clock, actor, write, identity, edit, gesture };
}

it('the target-owned debounce follows the latest generation and is cancelled when its actor stops', async () => {
  const f = await fixture();
  f.edit('https://first.example');
  f.clock.increment(1199);
  expect(f.write).not.toHaveBeenCalled();
  f.edit('https://latest.example');
  f.clock.increment(1199);
  expect(f.write).not.toHaveBeenCalled();
  f.clock.increment(1);
  await flush();
  expect(f.write).toHaveBeenCalledExactlyOnceWith('user-1', expect.objectContaining({ mutation: expect.objectContaining({
    authored: expect.objectContaining({ base_url: 'https://latest.example' }),
  }) }));
  f.edit('https://stopped.example');
  f.actor.stop();
  f.clock.increment(1200);
  await flush();
  expect(f.write).toHaveBeenCalledTimes(1);
});

it('credential blur, hold and cancellation never retain a timer outside the unit actor', async () => {
  const f = await fixture();
  f.edit('https://held.example', 'blur');
  f.clock.increment(10000);
  expect(f.write).not.toHaveBeenCalled();
  f.gesture('flush');
  await flush();
  expect(f.write).toHaveBeenCalledTimes(1);
  f.edit('https://menu.example');
  f.gesture('hold');
  f.clock.increment(10000);
  expect(f.write).toHaveBeenCalledTimes(1);
  f.gesture('cancel');
  f.gesture('flush');
  await flush();
  expect(f.write).toHaveBeenCalledTimes(1);
});

it('a ready generation survives detach, waits for native observation, and submits once after reattach', async () => {
  const f = await fixture();
  let release!: (value: WriteOutcome) => void;
  f.write.mockImplementationOnce(() => new Promise(resolve => { release = resolve; }));
  f.edit('https://first.example', 'now');
  await flush();
  f.edit('https://later.example', 'now');
  f.actor.send({ type: 'DETACH' });
  f.source.user.revision = 'user-2';
  release({ acknowledgement: { revision: 'user-2' } });
  await flush();
  expect(f.write).toHaveBeenCalledTimes(1);
  expect(f.actor.getSnapshot().context.units[f.identity].getSnapshot().context.draft?.value)
    .toMatchObject({ base_url: 'https://later.example' });
  f.actor.send({ type: 'ATTACH' });
  await flush();
  expect(f.write).toHaveBeenCalledTimes(2);
  expect(f.write.mock.calls[1]).toEqual(['user-2', expect.objectContaining({ mutation: expect.objectContaining({
    authored: expect.objectContaining({ base_url: 'https://later.example' }),
  }) })]);
  f.clock.increment(10000);
  await flush();
  expect(f.write).toHaveBeenCalledTimes(2);
});

it('a queued Provider save cannot erase another editor\'s completion before it is rendered', async () => {
  const f = await fixture();
  let release!: (value: WriteOutcome) => void;
  let releaseProvider!: (value: WriteOutcome) => void;
  f.write.mockImplementationOnce(() => new Promise(resolve => { release = resolve; }));
  f.write.mockImplementationOnce(() => new Promise(resolve => { releaseProvider = resolve; }));
  const mutation: SourceMutation = { kind: 'config', mutation: { unit: 'native_tools', authored: ['read'] } };
  const identity = JSON.stringify({ kind: 'config', mutation: { unit: 'native_tools', authored: null } });
  const selector = revisionSelector(mutation);
  f.actor.send({ type: 'UNIT.EDIT', identity, selector, revision: 'user-1', value: ['read'] });
  f.actor.send({ type: 'UNIT.SUBMIT', identity, selector, revision: 'user-1', mutation });
  f.edit('https://queued.example', 'now');
  await flush();
  f.source.user.revision = 'user-2';
  release({ acknowledgement: { revision: 'user-2' } });
  await flush();
  expect(f.write).toHaveBeenCalledTimes(2);
  expect(f.actor.getSnapshot().context.completions).toEqual({ [identity]: 1 });
  releaseProvider({ acknowledgement: { revision: 'user-2' } });
  await flush();
  expect(f.actor.getSnapshot().context.completions).toEqual({ [identity]: 1, [f.identity]: 2 });
  f.actor.send({ type: 'UNIT.EDIT', identity, selector, revision: 'user-2', value: [] });
  expect(f.actor.getSnapshot().context.completions).toEqual({ [f.identity]: 2 });
});
