export const WIDTH_PREFERENCE_KEY = 'rustx-conversation-width-v1';
export const WIDTH_MIN = 640;
export const WIDTH_EDGE = 176;
export function readWidthPreference(): number | undefined {
  try {
    const raw = globalThis.localStorage?.getItem(WIDTH_PREFERENCE_KEY);
    if (!raw || !/^\d+(\.\d+)?$/.test(raw)) return;
    const value = Number(raw);
    return Number.isFinite(value) && value >= WIDTH_MIN && value <= 10000 ? value : undefined;
  } catch { return; }
}
export function saveWidthPreference(value: number) {
  try { globalThis.localStorage?.setItem(WIDTH_PREFERENCE_KEY, String(value)); } catch { /* Presentation remains usable with an in-memory preference. */ }
}
export function displayedWidth(column: number, preference?: number) {
  const maximum = Math.max(0, column - (column >= WIDTH_MIN + WIDTH_EDGE ? WIDTH_EDGE : 48));
  return Math.max(0, Math.min(maximum, Math.max(WIDTH_MIN, preference ?? Math.max(680, Math.min(column * .64, 920)))));
}
