/** Round to 2 decimals so emitted path data stays compact and stable. */
export function r2(n: number): number {
  return Math.round(n * 100) / 100;
}

export function toNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

/** Expand a collapsed or tight domain so marks never sit on the frame. */
export function paddedDomain(lo: number, hi: number, ratio: number): [number, number] {
  if (lo === hi) {
    const delta = Math.max(1, Math.abs(lo) * 0.1);
    return [lo - delta, hi + delta];
  }
  const pad = (hi - lo) * ratio;
  return [lo - pad, hi + pad];
}

/** "Nice" 1/2/2.5/5×10ⁿ tick values covering [lo, hi], ~5 intervals. */
export function niceTicks(lo: number, hi: number): number[] {
  if (!Number.isFinite(lo) || !Number.isFinite(hi) || hi <= lo) return [];
  const raw = (hi - lo) / 5;
  if (!Number.isFinite(raw) || raw <= 0) return [];
  const mag = 10 ** Math.floor(Math.log10(raw));
  const norm = raw / mag;
  const step = (norm <= 1 ? 1 : norm <= 2 ? 2 : norm <= 2.5 ? 2.5 : norm <= 5 ? 5 : 10) * mag;
  if (!Number.isFinite(step) || step <= 0) return [];
  const ticks: number[] = [];
  for (let v = Math.ceil(lo / step - 1e-9) * step; v <= hi + step * 1e-9 && ticks.length < 24; v += step) {
    if (Number.isFinite(v)) ticks.push(Number(v.toPrecision(12)));
  }
  return ticks;
}

export function chartNumberFormatter(format: unknown, unit: unknown, ticks = false) {
  const currency = format === 'currency';
  const formatter = new Intl.NumberFormat(undefined, {
    style: currency ? 'currency' : 'decimal',
    currency: currency ? 'USD' : undefined,
    minimumFractionDigits: currency && !ticks ? 2 : 0,
    maximumFractionDigits: format === 'compact' ? 1 : 2,
    notation: format === 'compact' ? 'compact' : 'standard',
  });
  const suffix = format === 'percent' ? '%' : typeof unit === 'string' && unit ? ` ${unit}` : '';
  return (value: number) => formatter.format(value) + suffix;
}
