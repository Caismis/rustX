import { useSyncExternalStore } from 'react';
import type { TrajectoryTimelineMode } from './timeline';

const DURATION_KEY = 'rustx-trajectory-duration';
const STRING_WRAPPING_KEY = 'rustx-trajectory-json-string-wrapping';

const browserStorage = () => { try { return globalThis.localStorage; } catch { return undefined; } };

class BooleanPreferenceStore {
  private value: boolean;
  private listeners = new Set<() => void>();

  constructor(private readonly key: string, defaultValue: boolean) {
    this.value = defaultValue;
    try {
      const raw = browserStorage()?.getItem(key);
      if (raw !== null && raw !== undefined) {
        const parsed = JSON.parse(raw);
        if (typeof parsed === 'boolean') this.value = parsed;
      }
    } catch { /* Ignore unavailable or corrupt storage. */ }
  }

  getSnapshot = () => this.value;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };
  set = (next: boolean) => {
    if (this.value === next) return;
    this.value = next;
    try { browserStorage()?.setItem(this.key, JSON.stringify(next)); } catch { /* Ignore quota/storage errors. */ }
    this.listeners.forEach(listener => listener());
  };
}

export const trajectoryDurationStore = new BooleanPreferenceStore(DURATION_KEY, false);
export const trajectoryStringWrappingStore = new BooleanPreferenceStore(STRING_WRAPPING_KEY, false);

export function useTrajectoryDurationMode(): [TrajectoryTimelineMode, (mode: TrajectoryTimelineMode) => void] {
  const actualDuration = useSyncExternalStore(trajectoryDurationStore.subscribe, trajectoryDurationStore.getSnapshot);
  return [
    actualDuration ? 'duration' : 'sequence',
    (mode: TrajectoryTimelineMode) => trajectoryDurationStore.set(mode === 'duration'),
  ];
}

export function useTrajectoryStringWrapping(): [boolean, (wrapped: boolean) => void] {
  const wrapped = useSyncExternalStore(trajectoryStringWrappingStore.subscribe, trajectoryStringWrappingStore.getSnapshot);
  return [wrapped, trajectoryStringWrappingStore.set];
}
