import { expect, it } from 'vitest';
import { defaultEnabledSkillNames } from '../src/app/resources/shared/ResourceBrowser';

const skill = (name: string, enabled = true) => ({
  disable_model_invocation: false,
  enabled_by_default: enabled,
  catalog: { name },
});

it('keeps the fenced catalog names while a rescan is pending', () => {
  expect(defaultEnabledSkillNames({ skills: [skill('guide'), skill('copy', false)] })).toEqual(['guide']);
});

it('suppresses names on a catalog error and when the catalog is absent', () => {
  expect(defaultEnabledSkillNames({ error: 'failed', skills: [skill('stale')] })).toBeUndefined();
  expect(defaultEnabledSkillNames(undefined)).toBeUndefined();
});
