import { expect, it } from 'vitest';
import type { SessionModelConfig } from '../../protocol/app-server/v78';
import { nextModelSelection, selectionChanges } from '../src/bindings/model-catalog';
import { offersModelDefault, profileUnavailable, type ModelChoice, type ModelSelectionIntent } from '../src/presentation/agent/ModelSelect';

const chat = 'example/chat';
const pin = (profile: string): ModelSelectionIntent => ({ kind: 'profile', model: chat, profile });
const followDefault: ModelSelectionIntent = { kind: 'model-default', model: chat };

it('pinning the current default and returning to the Model default change configured values; repeats still produce complete selections', () => {
  // Following the default, which currently resolves to `balanced`.
  const following: SessionModelConfig = { model: chat };
  const pinned = nextModelSelection(following, pin('balanced'));
  expect(pinned).toEqual({ model: chat, profile: 'balanced' });
  expect(nextModelSelection(pinned, pin('balanced'))).toEqual(pinned);
  expect(nextModelSelection(pinned, followDefault)).toEqual({ model: chat });
  expect(nextModelSelection(following, followDefault)).toEqual(following);
  // A native `null` Profile is the same absent intent.
  expect(nextModelSelection({ model: chat, profile: null }, followDefault)).toEqual({ model: chat, profile: null });
  expect(nextModelSelection({ model: chat, profile: null }, pin('balanced'))).toEqual({ model: chat, profile: 'balanced' });
});

it('choosing the selected Model keeps a pinned Profile; only the default action clears it', () => {
  const fast: SessionModelConfig = { model: chat, profile: 'fast' };
  expect(selectionChanges(chat, 'fast', { kind: 'model', model: chat })).toBe(false);
  expect(nextModelSelection(fast, { kind: 'model', model: chat })).toEqual(fast);
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

it('a same-Model Profile change keeps every independent setting; a different Model resets primary settings and keeps the auxiliary policy', () => {
  const auxiliary: SessionModelConfig['auxiliary'] = { mode: 'explicit', model: 'example/summary', profile: { mode: 'explicit', id: 'short' }, request_params: { temperature: 0.1 }, max_output_tokens: 64 };
  const configured: SessionModelConfig = {
    model: chat,
    profile: 'fast',
    requestParams: { top_k: 40, nested: [null, { seed: 7 }] },
    maxOutputTokens: 512,
    auxiliary,
  };
  const { profile: _, ...independent } = configured;
  expect(nextModelSelection(configured, { kind: 'model', model: chat })).toEqual(configured);
  expect(nextModelSelection(configured, pin('fast'))).toEqual(configured);
  expect(nextModelSelection(independent, followDefault)).toEqual(independent);
  expect(nextModelSelection(configured, pin('balanced'))).toEqual({ ...independent, profile: 'balanced' });
  expect(nextModelSelection(configured, followDefault)).toEqual(independent);
  // A different Model gets its own default Profile (or the chosen one), no
  // primary request overrides and no primary output limit; the independently
  // owned auxiliary policy is carried whole, by value.
  for (const [intent, primary] of [
    [{ kind: 'model', model: 'example/other' }, { model: 'example/other' }],
    [{ kind: 'profile', model: 'example/other', profile: 'fast' }, { model: 'example/other', profile: 'fast' }],
    [{ kind: 'model-default', model: 'example/other' }, { model: 'example/other' }],
  ] as [ModelSelectionIntent, SessionModelConfig][]) {
    const switched = nextModelSelection(configured, intent)!;
    expect(switched).toEqual({ ...primary, auxiliary });
    for (const field of ['requestParams', 'maxOutputTokens'] as const) expect(Object.hasOwn(switched, field)).toBe(false);
  }
  // A Session-mode auxiliary is carried the same way; an absent one stays absent.
  expect(nextModelSelection({ model: chat, auxiliary: { mode: 'session' } }, { kind: 'model', model: 'example/other' })).toEqual({ model: 'example/other', auxiliary: { mode: 'session' } });
  expect(Object.hasOwn(nextModelSelection({ model: chat, maxOutputTokens: 512 }, { kind: 'model', model: 'example/other' })!, 'auxiliary')).toBe(false);
  // No configured selection yet: the gesture is the whole selection.
  expect(nextModelSelection(undefined, pin('balanced'))).toEqual({ model: chat, profile: 'balanced' });
  expect(nextModelSelection(undefined, followDefault)).toEqual({ model: chat });
});

it('switching the primary Model agrees with the TUI `/model X` contract', () => {
  // The configured state of tui/test/commands.test.ts "changes the model only
  // through model_catalog_get + model_set". The TUI sends
  // `{ model, requestParams: {}, auxiliary }`; an omitted `requestParams`
  // is natively the same empty object.
  const auxiliary: SessionModelConfig['auxiliary'] = { mode: 'explicit', model: 'summary/model-s', profile: { mode: 'explicit', id: 'compact' }, request_params: { summary_tag: 'keep' }, max_output_tokens: 300 };
  const configured: SessionModelConfig = { model: 'alpha/model-a', profile: 'on', requestParams: { temperature: 0.2 }, maxOutputTokens: 777, auxiliary };
  const tui: SessionModelConfig = { model: 'beta/model-b', requestParams: {}, auxiliary };
  const { requestParams, ...native } = tui;
  expect(requestParams).toEqual({});
  expect(nextModelSelection(configured, { kind: 'model', model: 'beta/model-b' })).toEqual(native);
});

it('a pinned Profile the Model no longer declares is cleared by the explicit default action alone, keeping every independent setting', () => {
  const stale: SessionModelConfig = { model: chat, profile: 'fast', requestParams: { top_k: 40 }, maxOutputTokens: 1024, auxiliary: { mode: 'explicit', model: 'example/summary' } };
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
  // removes only the pin. Repeated choices still produce a native selection.
  expect(nextModelSelection(stale, { kind: 'model', model: chat })).toEqual(stale);
  const recovered = nextModelSelection(stale, followDefault)!;
  expect(recovered).toEqual({ model: chat, requestParams: { top_k: 40 }, maxOutputTokens: 1024, auxiliary: { mode: 'explicit', model: 'example/summary' } });
  expect(Object.hasOwn(recovered, 'profile')).toBe(false);
  expect(nextModelSelection(recovered, followDefault)).toEqual(recovered);
});


it('preserves explicit and follow-session auxiliary policy across primary model and profile selection', () => {
  for (const auxiliary of [
    { mode: 'session' as const },
    { mode: 'explicit' as const, model: 'example/title', profile: { mode: 'explicit' as const, id: 'short' }, request_params: { title_tag: ['exact', null] }, max_output_tokens: 128 },
  ]) {
    const configured: SessionModelConfig = { model: chat, auxiliary, requestParams: { temperature: 0.5 } };
    expect(nextModelSelection(configured, { kind: 'model', model: 'example/other' })).toEqual({ model: 'example/other', auxiliary });
    expect(nextModelSelection(configured, pin('balanced'))).toEqual({ ...configured, profile: 'balanced' });
  }
});

it.each(['explicit', 'session'] as const)('primary model and profile gestures preserve complete %s auxiliary policies', mode => {
  const auxiliary: SessionModelConfig['auxiliary'] = mode === 'session' ? { mode } : {
    mode, model: 'classify/model', profile: { mode: 'explicit', id: 'precise' }, request_params: { temperature: 0.1, seed: 41 }, max_output_tokens: 123,
  };
  const configured: SessionModelConfig = { model: chat, profile: 'high', requestParams: { temperature: 0.3 }, maxOutputTokens: 789, auxiliary };
  for (const intent of [
    { kind: 'model', model: 'example/other' },
    { kind: 'profile', model: 'example/other', profile: 'fast' },
    { kind: 'model-default', model: 'example/other' },
  ] satisfies ModelSelectionIntent[]) {
    expect(nextModelSelection(configured, intent)).toEqual({ model: 'example/other', ...(intent.kind === 'profile' ? { profile: 'fast' } : {}), auxiliary });
  }
  expect(nextModelSelection(configured, pin('balanced'))).toEqual({ ...configured, profile: 'balanced' });
  expect(nextModelSelection(configured, { kind: 'model', model: chat })).toEqual(configured);
  expect(nextModelSelection(configured, followDefault)).toEqual({ model: chat, requestParams: configured.requestParams, maxOutputTokens: 789, auxiliary });
});
