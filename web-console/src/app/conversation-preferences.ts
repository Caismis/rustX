import { useSyncExternalStore } from 'react';

export const TRANSCRIPT_MODES = ['compact', 'standard', 'detailed', 'verbose'] as const;
export type TranscriptMode = typeof TRANSCRIPT_MODES[number];
export const COLLAPSE_TIMINGS = ['completion', 'next-input'] as const;
export type CollapseTiming = typeof COLLAPSE_TIMINGS[number];
export const FOLDED_REPLIES = ['all', 'final'] as const;
export type FoldedReplies = typeof FOLDED_REPLIES[number];
export const PERFORMANCE_USAGE_MODES = ['compact', 'detailed'] as const;
export type PerformanceUsageMode = typeof PERFORMANCE_USAGE_MODES[number];

export interface ConversationPreferences {
  fontSize: number;
  transcriptMode: TranscriptMode;
  collapseTiming: CollapseTiming;
  foldedReplies: FoldedReplies;
  performanceUsage: PerformanceUsageMode;
  codingView: boolean;
}
export const CONVERSATION_PREFERENCE_KEY = 'rustx-conversation-preferences-v1';
const defaults: ConversationPreferences = {
  fontSize: 14,
  transcriptMode: 'detailed',
  collapseTiming: 'completion',
  foldedReplies: 'all',
  performanceUsage: 'detailed',
  codingView: true,
};
const browserStorage = () => { try { return globalThis.localStorage; } catch { return undefined; } };
/** Browser presentation only; does not alter native messages or agent configuration. */
export class ConversationPreferenceStore {
  private value = defaults;
  private listeners = new Set<() => void>();
  constructor(private storage: Pick<Storage, 'getItem' | 'setItem'> | undefined = browserStorage()) {
    try { this.update(JSON.parse(storage?.getItem(CONVERSATION_PREFERENCE_KEY) ?? '{}'), false); } catch { /* Use defaults for unavailable or invalid storage. */ }
  }
  getSnapshot = () => this.value;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  update(patch: Partial<ConversationPreferences>, persist = true) {
    if (!patch || typeof patch !== 'object') return;
    const next = { ...this.value };
    if (Number.isInteger(patch.fontSize) && patch.fontSize! >= 10 && patch.fontSize! <= 22) next.fontSize = patch.fontSize!;
    if (TRANSCRIPT_MODES.includes(patch.transcriptMode!)) next.transcriptMode = patch.transcriptMode!;
    if (COLLAPSE_TIMINGS.includes(patch.collapseTiming!)) next.collapseTiming = patch.collapseTiming!;
    if (FOLDED_REPLIES.includes(patch.foldedReplies!)) next.foldedReplies = patch.foldedReplies!;
    if (PERFORMANCE_USAGE_MODES.includes(patch.performanceUsage!)) next.performanceUsage = patch.performanceUsage!;
    if (typeof patch.codingView === 'boolean') next.codingView = patch.codingView;
    if (next.fontSize === this.value.fontSize
      && next.transcriptMode === this.value.transcriptMode
      && next.collapseTiming === this.value.collapseTiming
      && next.foldedReplies === this.value.foldedReplies
      && next.performanceUsage === this.value.performanceUsage
      && next.codingView === this.value.codingView) return;
    this.value = next;
    if (persist) try { this.storage?.setItem(CONVERSATION_PREFERENCE_KEY, JSON.stringify(next)); } catch { /* Preferences still apply for this page. */ }
    this.listeners.forEach(listener => listener());
  }
}
let owner: ConversationPreferenceStore | undefined;
export const conversationPreferences = () => owner ??= new ConversationPreferenceStore();
export function useConversationPreferences() {
  const store = conversationPreferences();
  return [useSyncExternalStore(store.subscribe, store.getSnapshot), store] as const;
}
