import { useSyncExternalStore } from 'react';
const key = 'rustx-html-interactive-v1';
const listeners = new Set<() => void>();
let value: boolean | undefined;
function snapshot() {
  if (value === undefined) {
    try { value = localStorage.getItem(key) === 'true'; } catch { value = false; }
  }
  return value;
}
function subscribe(listener: () => void) { listeners.add(listener); return () => { listeners.delete(listener); }; }
export function useInteractiveHtml() {
  const enabled = useSyncExternalStore(subscribe, snapshot);
  return [enabled, (next: boolean) => {
    value = next;
    try { localStorage.setItem(key, String(next)); } catch { /* The live choice still applies. */ }
    listeners.forEach(listener => listener());
  }] as const;
}
