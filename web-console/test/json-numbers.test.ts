import { expect, it } from 'vitest';
import { hasUnrepresentableProperty, parseTransportJson } from '../shared/json-numbers';

it.each([
  '{"x":9007199254740993}', '{"x":[-9007199254740993]}', '9007199254740992',
  '{"x":1.0}', '{"x":-0}',
])('retains source-token evidence for unsafe native Workflow arguments: %s', argumentsJson => {
  const target = parseTransportJson(`{"type":"workflow","pinned":{},"arguments":${argumentsJson}}`) as object;
  expect(hasUnrepresentableProperty(target, 'arguments')).toBe(true);
});

it('keeps safe nested argument boundaries editable even if another target property is unsafe', () => {
  const target = parseTransportJson('{"type":"workflow","pinned":{"large":9007199254740993},"arguments":{"ids":[9007199254740991,-9007199254740991],"fraction":0.25}}') as object;
  expect(hasUnrepresentableProperty(target, 'pinned')).toBe(true);
  expect(hasUnrepresentableProperty(target, 'arguments')).toBe(false);
});
