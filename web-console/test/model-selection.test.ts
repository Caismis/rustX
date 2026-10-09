import { expect, it } from 'vitest';
import type { SessionModelConfig } from '../../protocol/app-server/v39';
import { nextModelSelection, selectionChanges } from '../src/bindings/model-catalog';
import type { ModelSelectionIntent } from '../src/presentation/agent/ModelSelect';

const chat = 'example/chat';
const pin = (profile: string): ModelSelectionIntent => ({ kind: 'profile', model: chat, profile });
const followDefault: ModelSelectionIntent = { kind: 'model-default', model: chat };

it('pinning the current default and returning to the Model default are configured changes; repeats are no-ops', () => {
  // Following the default, which currently resolves to `balanced`.
  const following: SessionModelConfig = { model: chat };
  const pinned = nextModelSelection(following, pin('balanced'));
  expect(pinned).toEqual({ model: chat, profile: 'balanced' });
  expect(nextModelSelection(pinned, pin('balanced'))).toBeUndefined();
  expect(nextModelSelection(pinned, followDefault)).toEqual({ model: chat });
  expect(nextModelSelection(following, followDefault)).toBeUndefined();
  // A native `null` Profile is the same absent intent.
  expect(nextModelSelection({ model: chat, profile: null }, followDefault)).toBeUndefined();
  expect(nextModelSelection({ model: chat, profile: null }, pin('balanced'))).toEqual({ model: chat, profile: 'balanced' });
});

it('choosing the selected Model keeps a pinned Profile; only the default action clears it', () => {
  const fast: SessionModelConfig = { model: chat, profile: 'fast' };
  expect(selectionChanges(chat, 'fast', { kind: 'model', model: chat })).toBe(false);
  expect(nextModelSelection(fast, { kind: 'model', model: chat })).toBeUndefined();
  expect(nextModelSelection(fast, followDefault)).toEqual({ model: chat });
});

it('a single-Model catalog reaches every configured Profile state without switching Models', () => {
  const steps: [ModelSelectionIntent, SessionModelConfig][] = [
    [pin('fast'), { model: chat, profile: 'fast' }],
    [followDefault, { model: chat }],
    [pin('balanced'), { model: chat, profile: 'balanced' }],
    [followDefault, { model: chat }],
  ];
  let configured: SessionModelConfig = { model: chat };
  for (const [intent, expected] of steps) {
    configured = nextModelSelection(configured, intent)!;
    expect(configured).toEqual(expected);
  }
});

it('a same-Model Profile change keeps every independent setting; a different Model starts from its native defaults', () => {
  const configured: SessionModelConfig = {
    model: chat,
    profile: 'fast',
    requestParams: { top_k: 40, nested: [null, { seed: 7 }] },
    maxOutputTokens: 512,
    summaryModel: { mode: 'explicit', model: 'example/summary', profile: 'short', request_params: { temperature: 0.1 }, max_output_tokens: 64 },
  };
  const { profile: _, ...independent } = configured;
  expect(nextModelSelection(configured, pin('balanced'))).toEqual({ ...independent, profile: 'balanced' });
  expect(nextModelSelection(configured, followDefault)).toEqual(independent);
  expect(nextModelSelection(configured, { kind: 'model', model: 'example/other' })).toEqual({ model: 'example/other' });
  expect(nextModelSelection(configured, { kind: 'profile', model: 'example/other', profile: 'fast' })).toEqual({ model: 'example/other', profile: 'fast' });
  expect(nextModelSelection(configured, { kind: 'model-default', model: 'example/other' })).toEqual({ model: 'example/other' });
  // No configured selection yet: the gesture is the whole selection.
  expect(nextModelSelection(undefined, pin('balanced'))).toEqual({ model: chat, profile: 'balanced' });
  expect(nextModelSelection(undefined, followDefault)).toEqual({ model: chat });
});
