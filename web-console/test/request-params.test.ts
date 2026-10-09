import { expect, it } from 'vitest';
import { exactJsonNumber, formatRequestParams, parseRequestParams, sameJson } from '../src/app/settings/forms/request-params';

it('parses exactly one JSON object and keeps nested values, arrays and null', () => {
  expect(parseRequestParams('{"a": {"b": [1, null, {"c": null}]}, "d": null, "e": "x"}')).toEqual({
    ok: true, value: { a: { b: [1, null, { c: null }] }, d: null, e: 'x' },
  });
  for (const text of ['', '{', '{"a":1,}', '{} {}', "{'a': 1}"]) expect(parseRequestParams(text)).toEqual({ ok: false, error: { kind: 'syntax' } });
  for (const text of ['null', '[]', '[{"a":1}]', '1', 'true', '"text"']) expect(parseRequestParams(text)).toEqual({ ok: false, error: { kind: 'not_object' } });
});

it('rejects a repeated key at any depth, located by line and column and never by key text', () => {
  for (const [text, line, column] of [
    ['{"a":1,"a":2}', 1, 8],
    ['{"a":{"b":1,"b":2}}', 1, 13],
    ['{"list":[{"k":1},{"k":1,"k":2}]}', 1, 25],
    ['{"x y":{},"x y":{}}', 1, 11],
    ['{"\\u0061":1,"a":2}', 1, 13],
    ['{"q\\"uote":[[{"z":1,"z":1}]]}', 1, 21],
    ['{\n  "outer": {\n    "sk-SECRET": 1,\n    "sk-SECRET": 2\n  }\n}', 4, 5],
    ['{"🔑 SECRET [x]":1,"🔑 SECRET [x]":2}', 1, 19],
  ] as const) {
    const parsed = parseRequestParams(text);
    expect(parsed).toEqual({ ok: false, error: { kind: 'duplicate_key', line, column } });
    expect(JSON.stringify(parsed)).not.toMatch(/SECRET|"[a-z]"/);
  }
  // The same key in sibling objects or as a string value is not a repeat.
  expect(parseRequestParams('{"a":{"k":1},"b":{"k":1},"c":["a","a"],"d":"a"}').ok).toBe(true);
  expect(parseRequestParams('{"s":"{\\"a\\":1,\\"a\\":2}"}').ok).toBe(true);
});

it('refuses a number JSON.parse would round, on its text, before any value exists', () => {
  for (const [text, line, column] of [
    ['{"seed":9007199254740993}', 1, 9],
    ['{"seed":-9007199254740993}', 1, 9],
    // Exactly a binary64, but printed back as 1152921504606847000.
    ['{"seed":1152921504606846976}', 1, 9],
    ['{"t":0.12345678901234567890}', 1, 6],
    ['{"a":{"b":[1,{"c":[2.5,9007199254740993]}]}}', 1, 24],
    ['{\n  "a": [\n    -18446744073709551616\n  ]\n}', 3, 5],
  ] as const) expect(parseRequestParams(text)).toEqual({ ok: false, error: { kind: 'inexact_number', line, column } });
  // A digit inside a string is not a number.
  expect(parseRequestParams('{"s":"9007199254740993","k9007199254740993":1}').ok).toBe(true);
});

it('keeps every binary64-exact number exactly, whatever its spelling', () => {
  const parsed = parseRequestParams('{"a":9007199254740992,"b":-9007199254740991,"c":0.1,"d":1.50,"e":15e-1,"f":1e20,"g":1E+300,"h":5e-324,"i":-0,"j":0.0,"k":10000000000000000000,"l":3.141592653589793,"m":[{"n":-2.5e-7}],"o":5.357830195732913e-76}');
  expect(parsed.ok).toBe(true);
  if (!parsed.ok) return;
  expect(parsed.value.a).toBe(9007199254740992);
  expect(parsed.value.d).toBe(1.5);
  expect(parsed.value.k).toBe(1e19);
  // The structured value the browser submits prints back to the same numbers
  // (JSON has one zero: -0 is sent as 0).
  const sent = parseRequestParams(JSON.stringify(parsed.value));
  expect(sent.ok && JSON.stringify(sent.value)).toBe(JSON.stringify(parsed.value));
});

it('every number the native runtime emits survives the browser hop exactly', () => {
  // serde_json prints integers in full and binary64 values as their shortest
  // round trip. Native refuses everything else, so these are the literals a
  // browser can receive: each is exact, and what JSON.stringify sends back
  // denotes the very same value.
  for (const literal of ['9007199254740992', '-9007199254740991', '10000000000000000000', '0.1', '1.5', '1e300',
    '5.357830195732913e-76', '1.603964615428183e143', '-2.5e-7', '0', '-0.0', '1e21', '123456789']) {
    expect(exactJsonNumber(literal), literal).toBe(true);
    const sent = JSON.stringify(JSON.parse(literal));
    expect(exactJsonNumber(sent), sent).toBe(true);
    expect(Number(sent) === Number(literal)).toBe(true);
  }
  for (const literal of ['9007199254740993', '-9007199254740993', '1152921504606846976', '18446744073709551616', '0.12345678901234567890', '1e400', '-1e400'])
    expect(exactJsonNumber(literal), literal).toBe(false);
});

it('compares structure, not formatting or key order', () => {
  const left = parseRequestParams('{"a": [1, {"b": null}], "c": 2}');
  const right = parseRequestParams('{ "c":2,\n "a":[1,{"b":null}] }');
  expect(left.ok && right.ok && sameJson(left.value, right.value)).toBe(true);
  expect(sameJson({ a: null }, {})).toBe(false);
  expect(sameJson({ a: [1, 2] }, { a: [2, 1] })).toBe(false);
  expect(formatRequestParams({ a: null })).toBe('{\n  "a": null\n}');
});
