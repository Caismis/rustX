/** Strict parsing of one provider-native request-parameter object.
 *
 * The browser edits `request_params` as a whole JSON object and submits the
 * structured value; native Rust remains the authority on protected keys,
 * profile ownership and every other configuration semantic. This module only
 * decides what a browser can decide truthfully: whether text is exactly one
 * JSON object without repeated keys whose every number this browser holds
 * exactly. `JSON.parse` silently keeps the last of a repeated key and silently
 * rounds a number to binary64, so it alone is neither check: both are decided
 * on the text, before `JSON.parse` can lose anything.
 *
 * Failures are located by line and column of the text, never by key or value:
 * an object key is as opaque as a value and may itself hold a secret. */

export type RequestParams = Record<string, unknown>;

/** A location in the edited text: 1-based line, 1-based column. */
export interface TextLocation { line: number; column: number }

export type RequestParamsParse =
  | { ok: true; value: RequestParams }
  | { ok: false; error: { kind: 'syntax' } | { kind: 'not_object' }
    | ({ kind: 'duplicate_key' } & TextLocation) | ({ kind: 'inexact_number' } & TextLocation) };

export function parseRequestParams(text: string): RequestParamsParse {
  let value: unknown;
  // The parsed value escapes only once the text is proven exact, so nothing
  // `JSON.parse` rounded or overwrote can reach a draft.
  try { value = JSON.parse(text); } catch { return { ok: false, error: { kind: 'syntax' } }; }
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return { ok: false, error: { kind: 'not_object' } };
  const problem = scan(text);
  if (problem) return { ok: false, error: problem };
  return { ok: true, value: value as RequestParams };
}

/** The canonical display text of one structured object. */
export function formatRequestParams(value: RequestParams): string {
  return JSON.stringify(value, null, 2);
}

/** Structural equality of two JSON values: formatting and key order are not
 * semantics. */
export function sameJson(left: unknown, right: unknown): boolean {
  if (Object.is(left, right)) return true;
  if (typeof left !== 'object' || typeof right !== 'object' || left === null || right === null) return false;
  if (Array.isArray(left) !== Array.isArray(right)) return false;
  if (Array.isArray(left) && Array.isArray(right)) return left.length === right.length && left.every((item, index) => sameJson(item, right[index]));
  const a = left as RequestParams, b = right as RequestParams;
  const keys = Object.keys(a);
  return keys.length === Object.keys(b).length && keys.every(key => Object.hasOwn(b, key) && sameJson(a[key], b[key]));
}

/** Whether one JSON number literal means the same value at every hop: the
 * browser's binary64 reading prints back exactly the value the literal denotes
 * (the I-JSON rule of RFC 7493 §2.2, which native rustX enforces identically).
 * `9007199254740993`, `2^60` and over-precise decimals are not; spelling
 * (`1.50`, `15e-1`) is irrelevant. */
export function exactJsonNumber(literal: string): boolean {
  const binary = Number(literal);
  if (!Number.isFinite(binary)) return false;
  const value = decimalValue(literal);
  return value !== undefined && value === decimalValue(String(binary));
}

/** The exact value of a decimal literal as one comparable key, with zero
 * normalized to one value. */
function decimalValue(literal: string): string | undefined {
  const match = /^(-?)(\d*)(?:\.(\d*))?(?:[eE]([+-]?\d+))?$/.exec(literal);
  if (!match) return undefined;
  const [, sign, integer = '', fraction = '', exponent = '0'] = match;
  const digits = `${integer}${fraction}`;
  const leading = digits.replace(/^0+/, '');
  const significant = leading.replace(/0+$/, '');
  if (significant === '') return '0';
  const scale = BigInt(exponent) - BigInt(fraction.length) + BigInt(leading.length - significant.length);
  return `${sign}${significant}e${scale}`;
}

/** The first repeated object key or inexact number literal of already
 * syntactically valid JSON text, located by line and column. */
function scan(text: string): ({ kind: 'duplicate_key' | 'inexact_number' } & TextLocation) | undefined {
  type Frame = { kind: 'object'; keys: Set<string>; expectKey: boolean } | { kind: 'array' };
  const stack: Frame[] = [];
  const at = (offset: number, kind: 'duplicate_key' | 'inexact_number') => {
    const lines = text.slice(0, offset).split('\n');
    // Columns count characters (code points), as an editor shows them.
    return { kind, line: lines.length, column: [...lines.at(-1)!].length + 1 };
  };
  for (let index = 0; index < text.length; index++) {
    const char = text[index]!;
    if (char === '"') {
      let end = index + 1;
      while (text[end] !== '"') end += text[end] === '\\' ? 2 : 1;
      const top = stack.at(-1);
      if (top?.kind === 'object' && top.expectKey) {
        const key = JSON.parse(text.slice(index, end + 1)) as string;
        if (top.keys.has(key)) return at(index, 'duplicate_key');
        top.keys.add(key);
        top.expectKey = false;
      }
      index = end;
    } else if (char === '-' || (char >= '0' && char <= '9')) {
      let end = index;
      while (end < text.length && /[0-9+\-.eE]/.test(text[end]!)) end++;
      if (!exactJsonNumber(text.slice(index, end))) return at(index, 'inexact_number');
      index = end - 1;
    } else if (char === '{') {
      stack.push({ kind: 'object', keys: new Set(), expectKey: true });
    } else if (char === '[') {
      stack.push({ kind: 'array' });
    } else if (char === '}' || char === ']') {
      stack.pop();
    } else if (char === ',') {
      const top = stack.at(-1);
      if (top?.kind === 'object') top.expectKey = true;
    }
  }
  return undefined;
}
