import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import {test} from 'node:test';
import {checkpointSnapshot, foldRuntimeEvent, observationCheckpoint} from '../../protocol/app-server/projection.ts';
import {deltaEvent, initialLiterals, advanceLiterals} from '../../protocol/app-server/test-support/projection-delta.ts';
import type {Notification, RuntimeClientEvent, RuntimeClientCheckpoint} from '../../protocol/app-server/v79.ts';
import {replaceFromSnapshot, reduce} from '../src/presentation/projection.ts';

for (const name of ['incremental-native', 'incremental-canonical-native']) test(`terminal full and delta rendering agree at every native state: ${name}`, () => {
  const capture = JSON.parse(readFileSync(new URL(`../../web-console/test/fixtures/${name}.json`, import.meta.url), 'utf8'));
  for (const item of capture.cases ?? [capture]) {
    const initial = item.initial.result.checkpoint as RuntimeClientCheckpoint;
    let native = checkpointSnapshot(structuredClone(initial)), cursor = initial.cursor;
    let literal=initialLiterals(initial);
    let full = replaceFromSnapshot(checkpointSnapshot(structuredClone(initial)), cursor);
    let incremental = replaceFromSnapshot(checkpointSnapshot(structuredClone(initial)), cursor);
    for (const frame of item.events as Extract<Notification, {method: 'session/event'}>[]) {
      const event = deltaEvent(observationCheckpoint(native, cursor)!, frame.params.event, literal);
      literal=advanceLiterals(literal,frame.params.event);
      full = reduce(full, {cursor: frame.params.cursor, event: frame.params.event});
      incremental = reduce(incremental, {cursor: frame.params.cursor, event});
      assert.deepEqual(incremental, full);
      native = foldRuntimeEvent(native, frame.params.event); cursor = frame.params.cursor;
    }
  }
});

test('terminal rendering agrees at every actual native text/tool delta, including head eviction', () => {
  const cases = JSON.parse(gunzipSync(readFileSync(new URL('../../protocol/app-server/test-support/native-delta.json.gz', import.meta.url))).toString()) as {
    kind: string; initial: RuntimeClientCheckpoint;
    events: {cursor: string; event: RuntimeClientEvent; full: RuntimeClientEvent}[];
  }[];
  for (const item of cases) {
    let full = replaceFromSnapshot(checkpointSnapshot(structuredClone(item.initial)), item.initial.cursor);
    let incremental = replaceFromSnapshot(checkpointSnapshot(structuredClone(item.initial)), item.initial.cursor);
    for (const frame of item.events) {
      full = reduce(full, {cursor: frame.cursor, event: frame.full});
      incremental = reduce(incremental, {cursor: frame.cursor, event: frame.event});
      assert.deepEqual(incremental, full, `${item.kind} cursor ${frame.cursor}`);
    }
  }
});
