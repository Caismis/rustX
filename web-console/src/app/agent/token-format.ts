/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-chat token-format.ts, the StatsPills
// formatDuration and message-chrome formatTokensPerSecond: compact and exact
// token counts, an honest cache-hit share, compact durations and speed.
import type { Translate } from '../../locale/translation';

/**
 * Compact token count: 517 / 12.2K / 517K / 1.2M.
 * @param value - non-negative token count.
 * @param tx - locale seat.
 * @returns locale-owned compact display string.
 */
export function formatTokens(value: number, tx: Translate): string {
  const scaled = (candidate: number): string =>
    candidate >= 100 ? String(Math.round(candidate)) : String(Math.round(candidate * 10) / 10);
  if (value < 1_000) return String(value);
  if (value < 1_000_000) return tx('agent:usage.thousand', { value: scaled(value / 1_000) });
  return tx('agent:usage.million', { value: scaled(value / 1_000_000) });
}

/**
 * Exact integer token count with locale-owned digit grouping.
 * @param value - non-negative safe integer token count.
 * @param tx - locale seat.
 * @returns an unrounded display string.
 */
export function formatExactTokens(value: number, tx: Translate): string {
  const digits = String(value);
  const groups: string[] = [];
  for (let end = digits.length; end > 0; end -= 3) groups.unshift(digits.slice(Math.max(0, end - 3), end));
  return groups.join(tx('agent:usage.group-separator'));
}

/** Round a cache-read ratio to exact percentage units, with positive ties rounded up. */
function roundedPercentUnits(cacheReadTokens: number, denominator: number, decimalPlaces: 0 | 1): number {
  const scale = (decimalPlaces === 0 ? 1 : 10) * 100;
  const doubledScale = scale * 2;
  const denominatorQuotient = Math.floor(denominator / doubledScale);
  const denominatorRemainder = denominator % doubledScale;
  let lower = 0;
  let upper = scale;
  while (lower < upper) {
    const candidate = Math.floor((lower + upper + 1) / 2);
    const factor = candidate * 2 - 1;
    const threshold = factor * denominatorQuotient + Math.ceil(factor * denominatorRemainder / doubledScale);
    if (cacheReadTokens >= threshold) lower = candidate;
    else upper = candidate - 1;
  }
  return lower;
}

function displayPercentUnits(units: number, decimalPlaces: 0 | 1): string {
  if (decimalPlaces === 0) return String(units);
  const whole = Math.floor(units / 10);
  const tenths = units % 10;
  return tenths === 0 ? String(whole) : `${whole}.${tenths}`;
}

/**
 * Display-ready cache-hit share without rounding a partial hit to 100%.
 * @param cacheReadTokens - exact prompt tokens served from cache.
 * @param promptTokens - exact aggregate prompt tokens.
 * @param decimalPlaces - ordinary-ratio precision; partial hits that would
 * round to 100 use enough additional precision to stay honest.
 * @returns percentage text, or null when there was no prompt input.
 */
export function formatCacheHitPercent(cacheReadTokens: number, promptTokens: number, decimalPlaces: 0 | 1 = 0): string | null {
  if (promptTokens === 0) return null;
  const missedInputTokens = promptTokens - cacheReadTokens;
  if (missedInputTokens === 0) return '100';
  const roundedUnits = roundedPercentUnits(cacheReadTokens, promptTokens, decimalPlaces);
  const fullHitUnits = decimalPlaces === 0 ? 100 : 1_000;
  if (roundedUnits < fullHitUnits) return displayPercentUnits(roundedUnits, decimalPlaces);
  let distinguishingPlaces = 1;
  let scaledDoubleGap = missedInputTokens * 200;
  const denominatorTens = Math.floor(promptTokens / 10);
  while (scaledDoubleGap <= denominatorTens) {
    scaledDoubleGap *= 10;
    distinguishingPlaces += 1;
  }
  const denominatorOnes = promptTokens % 10;
  let roundedLoss = 5;
  for (let loss = 1; loss < 5; loss += 1) {
    const factor = loss * 2 + 1;
    const threshold = factor * denominatorTens + Math.floor(factor * denominatorOnes / 10);
    if (scaledDoubleGap <= threshold) {
      roundedLoss = loss;
      break;
    }
  }
  return `99.${'9'.repeat(distinguishingPlaces - 1)}${10 - roundedLoss}`;
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
