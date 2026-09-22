import { createContext } from 'react';
import type { SourceSettings } from '../../../../protocol/app-server/v18';
/** Durable per-unit editor transaction state.
 *
 * The UI's editing intent is not the same fact as "a value override exists": a
 * clean Remove can pin the exact CAS base revision while authoring no value.
 * This record owns both, so a remount never silently adopts a newer revision
 * that the user has not reviewed. It is unsaved browser intent plus the exact
 * native revision the next mutation is fenced on; it is never persisted, merged
 * with sources, or used as runtime state. */
export interface UnitEditState {
  /** The browser's value override. Present only after an explicit Override or edit. */
  draft?: { value: unknown };
  /** The exact CAS base revision the next mutation is fenced on. */
  base: string;
  /** True once `base` is pinned by an edit or a mutation attempt. While pinned,
   * an observed revision never advances `base`; only Discard or the explicit
   * reviewed-revision gesture moves it. */
  pinned: boolean;
  /** Revision acknowledged by the last successful save, awaiting projection. */
  committed?: string;
  /** Pre-save revision of the last acknowledged save. While the projection
   * still carries exactly it, the source is merely unobserved, not changed. */
  savedFrom?: string;
}
export const EditorStateContext = createContext<Map<string, UnitEditState> | undefined>(undefined);
export const SourceContext = createContext<SourceSettings | undefined>(undefined);
