import { useState, useSyncExternalStore } from 'react';
import { AppServerClient } from '../../client/app-server';
import { RpcFailure } from '../../client/errors';
import type { Observation } from '../../client/session-lifecycle/port';
import { message as uiMessage, type DisplayText } from '../../locale/translation';

type Controls = { client?: AppServerClient; sessionId?: string };
const noSubscription = () => () => {};

/** Typed domain failures are presentation copy; opaque native diagnostics stay raw. */
function activityFailure(cause: unknown): DisplayText {
  const data = cause instanceof RpcFailure ? cause.error.data : undefined;
  switch (data?.kind) {
    case 'agent_not_delivered': return uiMessage('common:activity.not-delivered', { id: data.agent_id });
    case 'agent_delivery_unknown': return uiMessage('common:activity.delivery-unknown', { id: data.agent_id });
    case 'job_publication_abandoned': return uiMessage('common:activity.publication-abandoned', { id: data.job_id });
    case 'agent_settlement': return uiMessage('common:activity.agent-unresolved', { id: data.agent_id });
    default: return cause instanceof Error ? cause.message : String(cause);
  }
}

/** Request presentation only: native state always comes from the next snapshot.
 * A lost response is never retried, and a changed attachment cannot adopt it. */
export function useActivityRequest({ client, sessionId }: Controls) {
  const view = useSyncExternalStore(client?.subscribe ?? noSubscription, () => sessionId ? client?.getSnapshot().views[sessionId] : undefined);
  const proof = view?.attachmentObservation;
  const [state, setState] = useState<{ operation: { proof: Observation }; pending: boolean; error?: DisplayText }>();
  const visible = !!client && !!sessionId && client.isAttachmentObservationCurrent(sessionId, proof) && state?.operation.proof === proof;
  const pending = visible && !!state?.pending, error = visible ? state?.error : undefined;
  const enabled = !!client && !!sessionId && client.isAttachmentControlCurrent(sessionId, proof);
  async function run(operation: (client: AppServerClient, target: ReturnType<AppServerClient['target']>, current: () => boolean) => Promise<void>, refreshAfter = true) {
    if (!enabled || pending || !client || !sessionId || !proof) return false;
    const target = client.target(sessionId), owner = { proof };
    const current = () => client.isAttachmentObservationCurrent(sessionId, owner.proof);
    if (!current()) return false;
    setState({ operation: owner, pending: true });
    try { await operation(client, target, current); if (refreshAfter && current()) await client.refresh(sessionId); return current(); }
    catch (cause) { if (current()) setState(previous => current() && previous?.operation === owner ? { ...previous, error: activityFailure(cause) } : previous); return false; }
    finally { setState(previous => previous?.operation === owner ? { ...previous, pending: false } : previous); }
  }
  return { run, disabled: !enabled || pending, pending, error };
}
