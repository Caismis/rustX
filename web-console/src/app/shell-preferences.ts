import type { SidebarPreference } from '../presentation/layout/AppFrame';
const KEY = 'rustx-shell-layout-v1';

/** Geometry is optional browser-local presentation metadata, never authority. */
export function readShellLayout(): Partial<SidebarPreference> {
  try { return JSON.parse(localStorage.getItem(KEY) ?? '{}') ?? {}; } catch { return {}; }
}
export function saveShellLayout(value: SidebarPreference): void {
  try { localStorage.setItem(KEY, JSON.stringify(value)); } catch { /* Optional preference. */ }
}
