// @vitest-environment node
import { readFileSync } from 'node:fs';
import { expect, it } from 'vitest';
import type { Notification, RuntimeClientSnapshot } from '../../protocol/app-server/v26';
import { foldRuntimeEvent } from '../../protocol/app-server/projection';

it('native snapshot C + captured contiguous wire events = independently acquired native snapshot N', () => {
  const capture = JSON.parse(readFileSync('test/fixtures/incremental-native.json', 'utf8')) as {
    initial: { result: { snapshot: RuntimeClientSnapshot; cursor: string } };
    expected: { result: { snapshot: RuntimeClientSnapshot; cursor: string } };
    events: Extract<Notification, { method: 'session/event' }>[];
  };
  let state = capture.initial.result.snapshot, cursor = BigInt(capture.initial.result.cursor);
  for (const item of capture.events) {
    expect(BigInt(item.params.cursor)).toBe(cursor + 1n);
    state = foldRuntimeEvent(state, item.params.event); cursor++;
  }
  // Trace and explicit lifecycle-interest repairs have their own RPC/read cursor.
  // All Session fields, including transcript decorations/statistics and occupancy,
  // remain in this comparison. The expected value never passes through the fold.
  const session = ({ trace: _trace, trace_updates: _updates, ...session }: RuntimeClientSnapshot) => session;
  expect(cursor).toBe(BigInt(capture.expected.result.cursor));
  expect(session(state)).toEqual(session(capture.expected.result.snapshot));
});
