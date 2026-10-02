import { useSyncExternalStore } from 'react';

export type BusyEnterBehavior = 'queue' | 'steer';
export const COMPOSER_PREFERENCE_KEY = 'rustx-composer-busy-enter-v1';
const browserStorage = () => { try { return globalThis.localStorage; } catch { return undefined; } };

/** Origin/device presentation only. No native client or Session configuration. */
export class ComposerPreferences {
  private value: BusyEnterBehavior = 'queue';
  private readonly listeners = new Set<() => void>();
  constructor(private readonly storage: Pick<Storage, 'getItem' | 'setItem'> | undefined = browserStorage()) {
    try { if (storage?.getItem(COMPOSER_PREFERENCE_KEY) === 'steer') this.value = 'steer'; } catch { /* Queue is the safe default. */ }
  }
  getSnapshot = () => this.value;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  setBusyEnter(value: BusyEnterBehavior) {
    if (value !== 'queue' && value !== 'steer') return;
    try { this.storage?.setItem(COMPOSER_PREFERENCE_KEY, value); } catch { /* Live use remains available. */ }
    if (this.value === value) return;
    this.value = value;
    for (const listener of this.listeners) listener();
  }
}
let owner: ComposerPreferences | undefined;
export const composerPreferences = () => owner ??= new ComposerPreferences();
export function useBusyEnter() {
  const preference = composerPreferences();
  return [useSyncExternalStore(preference.subscribe, preference.getSnapshot), preference] as const;
}
