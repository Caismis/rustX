/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
/**
 * Normal column geometry: the right column shrinks, then loses its track,
 * before the center drops below its minimum. The sidebar never concedes here;
 * AppFrame supplies its effective preference after responsive collapse.
 */

/** Resolved widths for one frame. */
export interface Columns { primary: number; sidebar: number; center: number; rightbar: number }

/** Center width protected while the normal right column is open. */
export const CENTER_MIN = 400
/** Sidebar drag clamp floor. */
export const SIDEBAR_MIN = 264
/** Sidebar drag clamp ceiling. */
export const SIDEBAR_MAX = 420
/** Sidebar width before any user drag. */
export const SIDEBAR_DEFAULT = 280
/** Permanent primary navigation, independent of the Home sidebar. */
export const PRIMARY_RAIL_WIDTH = 56
/** Viewport width below which the Home sidebar auto-collapses (deepsuite
 * LG breakpoint); below it, manual expansion overlays the center instead of
 * consuming another grid track. */
export const SIDEBAR_AUTO_COLLAPSE = 1024
/** Right column drag clamp floor. */
export const RIGHTBAR_MIN = 300
/** Maximum normal right panel width as a fraction of the frame. */
export const RIGHTBAR_MAX_RATIO = 0.7
/** First-open right panel preference as a fraction of the frame. */
export const RIGHTBAR_DEFAULT_RATIO = 0.45

/**
 * Clamp a panel width into its contract range.
 * @param px - requested width.
 * @param min - range lower bound.
 * @param max - range upper bound.
 * @returns the clamped width.
 */
export function clampWidth(px: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, Math.round(px)))
}

/**
 * Solve the permanent rail and three optional/content column widths.
 * @param viewport - available frame width in px.
 * @param sidebar - sidebar width preference in px (0 = closed).
 * @param rightbar - requested right panel width in px (0 = no track).
 * @returns actual widths after shrinking or removing the right track; only
 *   without that track may the center fall below its minimum, down to zero.
 */
export function computeColumns(viewport: number, sidebar: number, rightbar: number): Columns {
  const s = sidebar === 0 ? 0 : clampWidth(sidebar, SIDEBAR_MIN, SIDEBAR_MAX)
  const available = viewport - PRIMARY_RAIL_WIDTH - s - CENTER_MIN
  const r = rightbar === 0 || available < RIGHTBAR_MIN
    ? 0
    : Math.min(available, clampWidth(rightbar, RIGHTBAR_MIN, viewport * RIGHTBAR_MAX_RATIO))
  return { primary: PRIMARY_RAIL_WIDTH, sidebar: s, center: Math.max(0, viewport - PRIMARY_RAIL_WIDTH - s - r), rightbar: r }
}
