import { expect, it } from 'vitest';
import { formatRequestParams, parseRequestParams, sameJson } from '../src/app/settings/forms/request-params';

it('parses exactly one JSON object and keeps nested values, arrays and null', () => {
  expect(parseRequestParams('{"a": {"b": [1, null, {"c": null}]}, "d": null, "e": "x"}')).toEqual({
    ok: true, value: { a: { b: [1, null, { c: null }] }, d: null, e: 'x' },
  });
  for (const text of ['', '{', '{"a":1,}', '{} {}', "{'a': 1}"]) expect(parseRequestParams(text)).toEqual({ ok: false, error: { kind: 'syntax' } });
  for (const text of ['null', '[]', '[{"a":1}]', '1', 'true', '"text"']) expect(parseRequestParams(text)).toEqual({ ok: false, error: { kind: 'not_object' } });
});

it('rejects a repeated key at any depth with a native-shaped path', () => {
  for (const [text, path] of [
    ['{"a":1,"a":2}', '$.a'],
    ['{"a":{"b":1,"b":2}}', '$.a.b'],
    ['{"list":[{"k":1},{"k":1,"k":2}]}', '$.list[1].k'],
    ['{"x y":{},"x y":{}}', '$["x y"]'],
    ['{"\\u0061":1,"a":2}', '$.a'],
    ['{"q\\"uote":[[{"z":1,"z":1}]]}', '$["q\\"uote"][0][0].z'],
  ] as const) expect(parseRequestParams(text)).toEqual({ ok: false, error: { kind: 'duplicate_key', path } });
  // The same key in sibling objects or as a string value is not a repeat.
  expect(parseRequestParams('{"a":{"k":1},"b":{"k":1},"c":["a","a"],"d":"a"}').ok).toBe(true);
  expect(parseRequestParams('{"s":"{\\"a\\":1,\\"a\\":2}"}').ok).toBe(true);
});

it('compares structure, not formatting or key order', () => {
  const left = parseRequestParams('{"a": [1, {"b": null}], "c": 2}');
  const right = parseRequestParams('{ "c":2,\n "a":[1,{"b":null}] }');
  expect(left.ok && right.ok && sameJson(left.value, right.value)).toBe(true);
  expect(sameJson({ a: null }, {})).toBe(false);
  expect(sameJson({ a: [1, 2] }, { a: [2, 1] })).toBe(false);
  expect(formatRequestParams({ a: null })).toBe('{\n  "a": null\n}');
});
