import { useSyncExternalStore } from 'react';
import type { SessionModelConfig } from '../../../protocol/app-server/v23';
import { type AppServerClient, sameTarget } from '../client/app-server';

const KEY = 'rustx-new-session-model-v1';
/** Browser product preference, scoped by App Server authority. It is only
 * copied into a new Session's intent; it never overrides native Session state.
 * Authored configuration and navigation storage are independent owners. */
export class NewSessionModelPreference {
  private values: Record<string, SessionModelConfig> = {};
  private listeners = new Set<() => void>();
  constructor(private readonly storage?: Pick<Storage, 'getItem' | 'setItem'>) {
    try {
      const value: unknown = JSON.parse(storage?.getItem(KEY) ?? '{}');
      if (value && typeof value === 'object') for (const [authority, selection] of Object.entries(value)) {
        if (selection && typeof selection === 'object' && typeof selection.model === 'string'
          && (selection.reasoningProfile === undefined || typeof selection.reasoningProfile === 'string')) {
          this.values[authority] = { model: selection.model, ...(selection.reasoningProfile === undefined ? {} : { reasoningProfile: selection.reasoningProfile }) };
        }
      }
    } catch { /* Unavailable browser storage means this preference is memory-only. */ }
  }
  read = (authority: string) => this.values[authority];
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  select(authority: string, selection: SessionModelConfig) {
    this.values = { ...this.values, [authority]: { model: selection.model, ...(selection.reasoningProfile == null ? {} : { reasoningProfile: selection.reasoningProfile }) } };
    try { this.storage?.setItem(KEY, JSON.stringify(this.values)); } catch { /* Still usable for this product lifetime. */ }
    for (const listener of this.listeners) listener();
  }
}
let owner: NewSessionModelPreference | undefined;
export function modelPreferences() {
  if (!owner) {
    let storage: Storage | undefined;
    try { storage = globalThis.localStorage; } catch { /* Storage disabled. */ }
    owner = new NewSessionModelPreference(storage);
  }
  return owner;
}
export function useModelPreference(authority: string) {
  const preference = modelPreferences();
  return useSyncExternalStore(preference.subscribe, () => preference.read(authority));
}

/** A successful native selection commits the product preference even if its
 * initiating control has since unmounted. Transport/attachment changes fence
 * the authority, not navigation. Rejections and lost acknowledgements never seed. */
export async function selectSessionModel(client: AppServerClient, id: string, selection: SessionModelConfig) {
  const { generation, endpoint } = client.getSnapshot();
  const target = client.target(id);
  await client.setAgentModel(id, selection);
  const current = client.getSnapshot();
  if (current.generation === generation && current.endpoint === endpoint && sameTarget(current.views[id]?.target, target)) {
    modelPreferences().select(endpoint ?? '', selection);
  }
}
