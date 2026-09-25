import { useMemo, useSyncExternalStore } from 'react';
import type { AppServerClient, ClientView } from './app-server';
import { deriveSessionProductState } from '../bindings/session-product';

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

/** Explicit shell subscription: no transcript, Trace, stream or Tool bodies.
 * Consumers of those facts subscribe at their own seats. */
function chrome(state: ClientView) {
  return {
    ...transportSelection(state), sessions: state.sessions, nextOffset: state.nextOffset,
    uncertain: state.uncertain, error: state.error, interactionOperations: state.interactionOperations,
    views: Object.values(state.views).map(view => ({
      id: view.id, target: view.target, summary: view.summary, settings: view.settings,
      attachment: view.attachment, attachmentIntent: view.attachmentIntent,
      deleting: view.deleting, deletionRecovery: view.deletionRecovery, recoveringDeletion: view.recoveringDeletion,
      modelMutation: view.modelMutation, cancellation: view.cancellation, inboundRequests: view.inboundRequests,
      submissions: view.submissions, error: view.error,
      product: deriveSessionProductState(state, view),
      snapshot: view.snapshot && {
        conversation: view.snapshot.conversation_id, shuttingDown: view.snapshot.shutting_down,
        durability: view.snapshot.durability_failure, phase: view.snapshot.attempt?.phase, attemptId: view.snapshot.attempt?.attempt_id,
        model: view.snapshot.model, resources: view.snapshot.resources?.revision,
        goal: !!view.snapshot.goal?.current && view.snapshot.goal.current.phase !== 'complete',
        pending: view.snapshot.inbound.pending?.length ?? 0,
        interactions: view.snapshot.pending_interactions,
      },
    })),
  };
}
export const selectClient = (state: ClientView) => state;
export const sameChrome = (a: ClientView, b: ClientView) => sameValue(chrome(a), chrome(b));
