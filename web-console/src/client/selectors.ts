import { useMemo, useSyncExternalStore } from 'react';
import type { AppServerClient, ClientView, SessionView } from './app-server';

/** Selected snapshots are cached across publications, not debounced. */
export function useClientSelector<T>(client: AppServerClient, select: (state: ClientView) => T, equal: (a: T, b: T) => boolean = Object.is): T {
  const read = useMemo(() => {
    let previous: ClientView | undefined;
    let selected: T;
    return () => {
      const state = client.getSnapshot();
      if (state !== previous) {
        const next = select(state);
        if (!previous || !equal(selected, next)) selected = next;
        previous = state;
      }
      return selected;
    };
  }, [client, select, equal]);
  return useSyncExternalStore(client.subscribe, read);
}

export const sameValue = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);
export const transportSelection = ({ connection, endpoint, generation, authorityRevision }: ClientView) => ({ connection, endpoint, generation, authorityRevision });

export type ShellView = Pick<ClientView, 'connection' | 'endpoint' | 'generation' | 'authorityRevision' | 'sessions' | 'nextOffset' | 'uncertain'> & {
  views: Readonly<Record<string, Pick<SessionView, 'id' | 'target' | 'summary' | 'settings' | 'attachment' | 'attachmentIntent' | 'deleting' | 'deletionRecovery' | 'recoveringDeletion'>>>;
};

/** Shell data contains no execution snapshot. Execution consumers subscribe locally. */
export const selectShell = (state: ClientView): ShellView => ({
  ...transportSelection(state), sessions: state.sessions, nextOffset: state.nextOffset,
  uncertain: state.uncertain.filter(item => !item.sessionId),
  views: Object.fromEntries(Object.values(state.views).map(view => [view.id, {
    id: view.id, target: view.target, summary: view.summary, settings: view.settings,
    attachment: view.attachment, attachmentIntent: view.attachmentIntent,
    deleting: view.deleting, deletionRecovery: view.deletionRecovery, recoveringDeletion: view.recoveringDeletion,
  }])),
});
export const selectClient = (state: ClientView) => state;
