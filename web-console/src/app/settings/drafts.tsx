import { createContext, useState, type ReactNode } from 'react';
// Only unsaved form intent. Never persisted, merged with sources, or used as runtime state.
export interface UnitDraft { value: unknown; base: string; dirty: boolean; committed?: string; }
export const DraftContext = createContext<Map<string, UnitDraft> | undefined>(undefined);
export function SettingsDrafts({ children }: { children: ReactNode }) {
  const [drafts] = useState(() => new Map<string, UnitDraft>());
  return <DraftContext value={drafts}>{children}</DraftContext>;
}
