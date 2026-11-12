// @vitest-environment node
import {expect, it} from 'vitest';
import {readFileSync} from 'node:fs';
import type {Notification, RuntimeClientCheckpoint} from '../../protocol/app-server/v79';
import {checkpointSnapshot, foldRuntimeEvent, observationCheckpoint, expandRuntimeEvent} from '../../protocol/app-server/projection';
import {deltaEvent, listDelta, initialLiterals, advanceLiterals} from '../../protocol/app-server/test-support/projection-delta';
import {ObservationChain} from '../../protocol/app-server/observation';

it.each(['incremental-native', 'incremental-canonical-native'])('full and delta folds agree at every native state (%s)', name => {
  const capture = JSON.parse(readFileSync(`test/fixtures/${name}.json`, 'utf8'));
  const cases = capture.cases ?? [capture];
  let retained = 0;
  for (const item of cases) {
    const initial = item.initial.result.checkpoint as RuntimeClientCheckpoint;
    let full = checkpointSnapshot(structuredClone(initial)), incremental = checkpointSnapshot(structuredClone(initial));
    let cursor = initial.cursor;
    let literal=initialLiterals(initial);
    for (const frame of item.events as Extract<Notification, {method: 'session/event'}>[]) {
      const base = observationCheckpoint(incremental, cursor)!;
      const delta = deltaEvent(base, frame.params.event, literal);
      expect(expandRuntimeEvent(incremental, delta)).toStrictEqual(frame.params.event);
      if (delta.type === 'projection_delta') retained += delta.metadata.content.slots.length - delta.metadata.content.values.length;
      literal=advanceLiterals(literal,frame.params.event);
      full = foldRuntimeEvent(full, frame.params.event);
      incremental = foldRuntimeEvent(incremental, delta);
      cursor = frame.params.cursor;
      expect(JSON.parse(JSON.stringify(observationCheckpoint(incremental, cursor)))).toStrictEqual(JSON.parse(JSON.stringify(observationCheckpoint(full, cursor))));
    }
  }
  expect(retained).toBeGreaterThan(0);
});

it('rejects stale bases and invalid slots before advancing Applied, then recovers from a complete checkpoint', () => {
  const capture = JSON.parse(readFileSync('test/fixtures/incremental-native.json', 'utf8'));
  const checkpoint = capture.initial.result.checkpoint as RuntimeClientCheckpoint;
  const state = checkpointSnapshot(checkpoint);
  const full = capture.events[0].params.event;
  const event = deltaEvent(checkpoint, full);
  if (event.type !== 'projection_delta') throw new Error('Expected atomic native transition');
  event.metadata.content.slots = [checkpoint.content.length + 100];
  const chain = new ObservationChain<typeof event>();
  chain.installCheckpoint(checkpoint.cursor); chain.beginSubscription(checkpoint.cursor); chain.acceptReceipt('r', checkpoint.cursor);
  expect(chain.deliver({registrationId: 'r', cursor: String(BigInt(checkpoint.cursor) + 1n), event}, fact => { foldRuntimeEvent(state, fact); })).toBe('recovery');
  expect(chain.applied).toBe(checkpoint.cursor);
  expect(chain.phase).toBe('resyncRequired');
  chain.installCheckpoint(checkpoint.cursor);
  const stale = {...event, metadata: {...event.metadata, base_cursor: String(BigInt(checkpoint.cursor) + 1n)}};
  expect(() => foldRuntimeEvent(state, stale)).toThrow('base mismatch');
  expect(listDelta([1, 2, 3], [3, 4, 2])).toEqual({slots: [2, 3, 1], values: [4]});
});

it('keeps independent scope and source-cut validation after reconstructing changed cut components', () => {
  const capture = JSON.parse(readFileSync('test/fixtures/incremental-native.json', 'utf8'));
  const checkpoint = capture.initial.result.checkpoint as RuntimeClientCheckpoint;
  const state = checkpointSnapshot(checkpoint);
  const event = deltaEvent(checkpoint, capture.events[0].params.event);
  if (event.type !== 'projection_delta') throw new Error('Expected atomic native transition');
  for (const scope of [{conversation_id:'foreign'}, {activation_id:'foreign'}]) {
    expect(() => expandRuntimeEvent(state, {...event, metadata:{...event.metadata,read_cut:{...event.metadata.read_cut,...scope}}})).toThrow('scope mismatch');
  }
});
