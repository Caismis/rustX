import { expect, it } from 'vitest';
import type { SessionModelConfig } from '../../protocol/app-server/v39';
import { nextModelSelection, selectionChanges } from '../src/bindings/model-catalog';
import { offersModelDefault, profileUnavailable, type ModelChoice, type ModelSelectionIntent } from '../src/presentation/agent/ModelSelect';

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

it('a same-Model Profile change keeps every independent setting; a different Model resets primary settings and keeps the Summary policy', () => {
  const summaryModel: SessionModelConfig['summaryModel'] = { mode: 'explicit', model: 'example/summary', profile: 'short', request_params: { temperature: 0.1 }, max_output_tokens: 64 };
  const configured: SessionModelConfig = {
    model: chat,
    profile: 'fast',
    requestParams: { top_k: 40, nested: [null, { seed: 7 }] },
    maxOutputTokens: 512,
    summaryModel,
  };
  const { profile: _, ...independent } = configured;
  expect(nextModelSelection(configured, pin('balanced'))).toEqual({ ...independent, profile: 'balanced' });
  expect(nextModelSelection(configured, followDefault)).toEqual(independent);
  // A different Model gets its own default Profile (or the chosen one), no
  // primary request overrides and no primary output limit; the independently
  // owned Summary policy is carried whole, by value.
  for (const [intent, primary] of [
    [{ kind: 'model', model: 'example/other' }, { model: 'example/other' }],
    [{ kind: 'profile', model: 'example/other', profile: 'fast' }, { model: 'example/other', profile: 'fast' }],
    [{ kind: 'model-default', model: 'example/other' }, { model: 'example/other' }],
  ] as [ModelSelectionIntent, SessionModelConfig][]) {
    const switched = nextModelSelection(configured, intent)!;
    expect(switched).toEqual({ ...primary, summaryModel });
    for (const field of ['requestParams', 'maxOutputTokens'] as const) expect(Object.hasOwn(switched, field)).toBe(false);
  }
  // A Session-mode Summary is carried the same way; an absent one stays absent.
  expect(nextModelSelection({ model: chat, summaryModel: { mode: 'session' } }, { kind: 'model', model: 'example/other' })).toEqual({ model: 'example/other', summaryModel: { mode: 'session' } });
  expect(Object.hasOwn(nextModelSelection({ model: chat, maxOutputTokens: 512 }, { kind: 'model', model: 'example/other' })!, 'summaryModel')).toBe(false);
  // No configured selection yet: the gesture is the whole selection.
  expect(nextModelSelection(undefined, pin('balanced'))).toEqual({ model: chat, profile: 'balanced' });
  expect(nextModelSelection(undefined, followDefault)).toEqual({ model: chat });
});

it('switching the primary Model agrees with the TUI `/model X` contract', () => {
  // The configured state of tui/test/commands.test.ts "changes the model only
  // through model_catalog_get + model_set". The TUI sends
  // `{ model, requestParams: {}, summaryModel }`; an omitted `requestParams`
  // is natively the same empty object.
  const summaryModel: SessionModelConfig['summaryModel'] = { mode: 'explicit', model: 'summary/model-s', profile: 'compact', request_params: { summary_tag: 'keep' }, max_output_tokens: 300 };
  const configured: SessionModelConfig = { model: 'alpha/model-a', profile: 'on', requestParams: { temperature: 0.2 }, maxOutputTokens: 777, summaryModel };
  const tui: SessionModelConfig = { model: 'beta/model-b', requestParams: {}, summaryModel };
  const { requestParams, ...native } = tui;
  expect(requestParams).toEqual({});
  expect(nextModelSelection(configured, { kind: 'model', model: 'beta/model-b' })).toEqual(native);
});

it('a pinned Profile the Model no longer declares is cleared by the explicit default action alone, keeping every independent setting', () => {
  const stale: SessionModelConfig = { model: chat, profile: 'fast', requestParams: { top_k: 40 }, maxOutputTokens: 1024, summaryModel: { mode: 'explicit', model: 'example/summary' } };
  const withoutProfiles: ModelChoice = { id: chat, profiles: [] };
  const withOthers: ModelChoice = { id: chat, profiles: [{ id: 'balanced', label: 'balanced' }], defaultProfile: 'balanced' };
  // Offered on the configured Model only while its pin is unavailable; no
  // default Profile is named for a Model without Profiles.
  expect(profileUnavailable(withoutProfiles, 'fast')).toBe(true);
  expect(profileUnavailable(withOthers, 'fast')).toBe(true);
  expect(profileUnavailable(withOthers, 'balanced')).toBe(false);
  expect(profileUnavailable(withoutProfiles, undefined)).toBe(false);
  expect(offersModelDefault(withoutProfiles, chat, 'fast')).toBe(true);
  expect(offersModelDefault(withoutProfiles, chat, undefined)).toBe(false);
  expect(offersModelDefault(withoutProfiles, 'example/other', 'fast')).toBe(false);
  expect(offersModelDefault(withOthers, chat, undefined)).toBe(true);
  // Choosing the selected Model keeps the stale pin; the default action
  // removes only the pin, and repeating it is a no-op.
  expect(nextModelSelection(stale, { kind: 'model', model: chat })).toBeUndefined();
  const recovered = nextModelSelection(stale, followDefault)!;
  expect(recovered).toEqual({ model: chat, requestParams: { top_k: 40 }, maxOutputTokens: 1024, summaryModel: { mode: 'explicit', model: 'example/summary' } });
  expect(Object.hasOwn(recovered, 'profile')).toBe(false);
  expect(nextModelSelection(recovered, followDefault)).toBeUndefined();
});
