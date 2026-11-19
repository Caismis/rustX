import { useSyncExternalStore } from 'react';
import type { AppServerClient } from '../../client/app-server';

export type JobGroup = 'live' | 'settled';

export interface JobsSessionState {
  cleared: ReadonlySet<string>;
  settledOpen: boolean | undefined;
  unfolded: Record<JobGroup, boolean>;
  acceptedStop: { key: string; current: () => boolean } | undefined;
}

const initialState = (): JobsSessionState => ({
  cleared: new Set(),
  settledOpen: undefined,
  unfolded: { live: false, settled: false },
  acceptedStop: undefined,
});

interface StoreEntry {
  state: JobsSessionState;
  listeners: Set<() => void>;
}

const stores = new WeakMap<AppServerClient, Map<string, StoreEntry>>();

function entryFor(client: AppServerClient, key: string): StoreEntry {
  let byKey = stores.get(client);
  if (!byKey) {
    byKey = new Map();
    stores.set(client, byKey);
  }
  let entry = byKey.get(key);
  if (!entry) {
    entry = { state: initialState(), listeners: new Set() };
    byKey.set(key, entry);
  }
  return entry;
}

function updateEntry(client: AppServerClient, key: string, updater: (current: JobsSessionState) => JobsSessionState) {
  const entry = entryFor(client, key);
  const next = updater(entry.state);
  if (next === entry.state) return;
  entry.state = next;
  for (const listener of entry.listeners) listener();
}

const noopSubscribe = () => () => {};
const FALLBACK_STATE = initialState();
const fallbackSnapshot = () => FALLBACK_STATE;

/**
 * Per-Session UI state for background jobs (cleared settled rows, settled fold,
 * per-group >5 folds, and accepted pending stop), retained while switching
 * subagents, views, or sessions under the same AppServerClient.
 */
export function useJobsSessionStore(client: AppServerClient | undefined, sessionKey: string | undefined) {
  const subscribe = client && sessionKey
    ? (listener: () => void) => {
        const entry = entryFor(client, sessionKey);
        entry.listeners.add(listener);
        return () => { entry.listeners.delete(listener); };
      }
    : noopSubscribe;
  const getSnapshot = client && sessionKey
    ? () => entryFor(client, sessionKey).state
    : fallbackSnapshot;
  const state = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);

  return {
    state,
    clearSettled: (ids: readonly string[]) => {
      if (!client || !sessionKey) return;
      updateEntry(client, sessionKey, current => ({
        ...current,
        cleared: new Set([...current.cleared, ...ids]),
        unfolded: { ...current.unfolded, settled: false },
      }));
    },
    setSettledOpen: (open: boolean) => {
      if (!client || !sessionKey) return;
      updateEntry(client, sessionKey, current => ({ ...current, settledOpen: open }));
    },
    setUnfolded: (group: JobGroup, value: boolean) => {
      if (!client || !sessionKey) return;
      updateEntry(client, sessionKey, current => ({
        ...current,
        unfolded: { ...current.unfolded, [group]: value },
      }));
    },
    setAcceptedStop: (acceptedStop: { key: string; current: () => boolean } | undefined) => {
      if (!client || !sessionKey) return;
      updateEntry(client, sessionKey, current => ({ ...current, acceptedStop }));
    },
  };
}
