/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-chat token-format.ts, the StatsPills
// formatDuration and message-chrome formatTokensPerSecond: compact and exact
// token counts, an honest cache-hit share, compact durations and speed.
import type { Translate } from '../../locale/translation';

/** Native decimal u64 evidence, or a finite locally calculated quantity. */
export type TokenCount = string | number | bigint;
const integer = (value: TokenCount): bigint => {
  if (typeof value === 'number' && !Number.isSafeInteger(value)) throw new Error('Token quantity is not exact.');
  return BigInt(value);
};

/**
 * Compact token count: 517 / 12.2K / 517K / 1.2M.
 * @param value - non-negative token count.
 * @param tx - locale seat.
 * @returns locale-owned compact display string.
 */
export function formatTokens(value: TokenCount, tx: Translate): string {
  const count = integer(value);
  const scaled = (unit: bigint): string => {
    const whole = count / unit;
    if (whole >= 100n) return String((count * 2n + unit) / (unit * 2n));
    const tenths = (count * 20n + unit) / (unit * 2n);
    return tenths % 10n === 0n ? String(tenths / 10n) : `${tenths / 10n}.${tenths % 10n}`;
  };
  if (count < 1_000n) return String(count);
  if (count < 1_000_000n) return tx('agent:usage.thousand', { value: scaled(1_000n) });
  return tx('agent:usage.million', { value: scaled(1_000_000n) });
}

/**
 * Exact integer token count with locale-owned digit grouping.
 * @param value - exact non-negative native integer token count.
 * @param tx - locale seat.
 * @returns an unrounded display string.
 */
export function formatExactTokens(value: TokenCount, tx: Translate): string {
  const digits = String(integer(value));
  const groups: string[] = [];
  for (let end = digits.length; end > 0; end -= 3) groups.unshift(digits.slice(Math.max(0, end - 3), end));
  return groups.join(tx('agent:usage.group-separator'));
}

/** Positive ties round up, without converting native quantities to floats. */
function roundedPercentUnits(cached: bigint, prompt: bigint, scale: bigint): bigint {
  return (cached * scale * 2n + prompt) / (prompt * 2n);
}
function displayPercentUnits(units: bigint, decimalPlaces: number): string {
  const scale = 10n ** BigInt(decimalPlaces);
  const whole = units / scale, fraction = String(units % scale).padStart(decimalPlaces, '0').replace(/0+$/, '');
  return fraction ? `${whole}.${fraction}` : String(whole);
}

/**
 * Display-ready cache-hit share without rounding a partial hit to 100%.
 * @param cacheReadTokens - exact prompt tokens served from cache.
 * @param promptTokens - exact aggregate prompt tokens.
 * @param decimalPlaces - ordinary-ratio precision; partial hits that would
 * round to 100 use enough additional precision to stay honest.
 * @returns percentage text, or null when there was no prompt input.
 */
export function formatCacheHitPercent(cacheReadTokens: TokenCount, promptTokens: TokenCount, decimalPlaces: 0 | 1 = 0): string | null {
  const cached = integer(cacheReadTokens), prompt = integer(promptTokens);
  if (prompt === 0n || cached < 0n || cached > prompt) return null;
  if (cached === prompt) return '100';
  // At most 22 decimal places distinguish a partial hit over the entire u64
  // domain. Retain the exact native input and make this display choice locally.
  for (let places: number = decimalPlaces; places <= 22; places++) {
    const scale = 100n * 10n ** BigInt(places);
    const units = roundedPercentUnits(cached, prompt, scale);
    if (units < scale) return displayPercentUnits(units, places);
  }
  throw new Error('Native token ratio exceeds its u64 display domain.');
}

/**
 * Compact duration: 45.2s under a minute, 2m42s from there on.
 * @param ms - duration in milliseconds.
 * @param tx - locale seat.
 */
export function formatDuration(ms: number, tx: Translate): string {
  const seconds = ms / 1_000;
  if (seconds < 60) return tx('agent:usage.seconds', { seconds: Math.round(seconds * 10) / 10 });
  const whole = Math.round(seconds);
  return tx('agent:usage.minutes', { minutes: Math.floor(whole / 60), seconds: whole % 60 });
}

/** Whole tokens per second from ten up, one decimal below. */
export function formatTokensPerSecond(tps: number): string {
  const clamped = Math.max(0, tps);
  return clamped >= 10 ? String(Math.round(clamped)) : String(Math.round(clamped * 10) / 10);
}
