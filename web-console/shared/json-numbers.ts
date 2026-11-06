/** Match native Workflow number semantics before JavaScript discards the
 * source token: safe decimal integers and finite non-integral floats survive,
 * whereas negative zero, unsafe integers and integral floats below 1e21 do not. */
export function representableNumber(token: string | undefined, value: number): boolean {
  if (token === undefined || !Number.isFinite(value) || Object.is(value, -0)) return false;
  if (/^-?\d+$/.test(token)) return Number.isSafeInteger(value);
  return !(Number.isInteger(value) && Math.abs(value) < 1e21);
}

export type JsonReading = { ok: true; value: unknown } | { ok: false; reason: 'syntax' | 'number' };
export function readJson(text: string): JsonReading {
  let exact = true;
  let value: unknown;
  try {
    value = JSON.parse(text, function (this: unknown, _key: string, parsed: unknown, context?: { source?: string }) {
      if (typeof parsed === 'number' && !representableNumber(context?.source, parsed)) exact = false;
      return parsed;
    } as (this: unknown, key: string, value: unknown) => unknown);
  } catch { return { ok: false, reason: 'syntax' }; }
  return exact ? { ok: true, value } : { ok: false, reason: 'number' };
}

// Evidence belongs to the decoded object, not its rounded numeric value. The
// WeakMap follows only the lifetime of the native response and does not alter
// its wire shape. A missing source-token API fails closed for numeric inputs.
const unrepresentable = new WeakMap<object, Set<string>>();
export function parseTransportJson(text: string): unknown {
  return JSON.parse(text, function (this: object, key: string, value: unknown, context?: { source?: string }) {
    const unsafe = typeof value === 'number' ? !representableNumber(context?.source, value)
      : !!value && typeof value === 'object' && (unrepresentable.get(value)?.size ?? 0) > 0;
    if (unsafe) {
      const fields = unrepresentable.get(this) ?? new Set<string>();
      fields.add(key); unrepresentable.set(this, fields);
    }
    return value;
  } as (this: object, key: string, value: unknown) => unknown);
}
export function hasUnrepresentableProperty(value: object, key: string): boolean {
  return unrepresentable.get(value)?.has(key) ?? false;
}
