/** Strict parsing of one provider-native request-parameter object.
 *
 * The browser edits `request_params` as a whole JSON object and submits the
 * structured value; native Rust remains the authority on protected keys,
 * profile ownership and every other configuration semantic. This module only
 * decides what a browser can decide truthfully: whether text is exactly one
 * JSON object without repeated keys. `JSON.parse` silently keeps the last of a
 * repeated key, so it alone is not that check. */

export type RequestParams = Record<string, unknown>;

export type RequestParamsParse =
  | { ok: true; value: RequestParams }
  | { ok: false; error: { kind: 'syntax' } | { kind: 'not_object' } | { kind: 'duplicate_key'; path: string } };

export function parseRequestParams(text: string): RequestParamsParse {
  let value: unknown;
  try { value = JSON.parse(text); } catch { return { ok: false, error: { kind: 'syntax' } }; }
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return { ok: false, error: { kind: 'not_object' } };
  const duplicate = duplicateKey(text);
  if (duplicate !== undefined) return { ok: false, error: { kind: 'duplicate_key', path: duplicate } };
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

/** The first repeated object key of already syntactically valid JSON text, as
 * a `$`-rooted path of keys and indices, matching native diagnostics. */
function duplicateKey(text: string): string | undefined {
  type Frame = { kind: 'object'; keys: Set<string>; path: string; key?: string; expectKey: boolean } | { kind: 'array'; path: string; index: number };
  const stack: Frame[] = [];
  const childPath = () => {
    const top = stack.at(-1);
    if (!top) return '$';
    return top.kind === 'object' ? `${top.path}${segment(top.key ?? '')}` : `${top.path}[${top.index}]`;
  };
  for (let at = 0; at < text.length; at++) {
    const char = text[at];
    if (char === '"') {
      let end = at + 1;
      while (text[end] !== '"') end += text[end] === '\\' ? 2 : 1;
      const top = stack.at(-1);
      if (top?.kind === 'object' && top.expectKey) {
        const key = JSON.parse(text.slice(at, end + 1)) as string;
        if (top.keys.has(key)) return `${top.path}${segment(key)}`;
        top.keys.add(key);
        top.key = key;
        top.expectKey = false;
      }
      at = end;
    } else if (char === '{') {
      stack.push({ kind: 'object', keys: new Set(), path: childPath(), expectKey: true });
    } else if (char === '[') {
      stack.push({ kind: 'array', path: childPath(), index: 0 });
    } else if (char === '}' || char === ']') {
      stack.pop();
    } else if (char === ',') {
      const top = stack.at(-1);
      if (top?.kind === 'object') top.expectKey = true;
      else if (top?.kind === 'array') top.index += 1;
    }
  }
  return undefined;
}

function segment(key: string): string {
  return /^[A-Za-z0-9_-]+$/.test(key) ? `.${key}` : `[${JSON.stringify(key)}]`;
}
