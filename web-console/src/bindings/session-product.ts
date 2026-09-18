import type { ClientView, SessionView } from '../client/app-server';

export type SessionRecovery = 'connect' | 'connection-settings' | 'open' | 'refresh';
export interface SessionProductState {
  status: 'idle' | 'working' | 'queued' | 'stopping' | 'connecting' | 'reconnect' | 'uncertain' | 'failure' | 'waiting';
  label?: string;
  detail?: string;
  severity: 'quiet' | 'warning' | 'error';
  recovery?: { action: SessionRecovery; label: string };
}

/** Pure display projection. Stale snapshots never imply current execution or
 * settlement. Recovery only names an existing client operation; no replay. */
export function deriveSessionProductState(state: Pick<ClientView, 'connection' | 'uncertain'>, view?: SessionView, sessionId = view?.id): SessionProductState {
  const connecting = ['connecting', 'reconnecting', 'resynchronizing'].includes(state.connection);
  const recovery: SessionProductState['recovery'] = connecting ? undefined
    : state.connection === 'incompatible' ? { action: 'connection-settings', label: 'Connection settings' }
    : state.connection !== 'connected' ? { action: 'connect', label: 'Reconnect' }
    : !view || view.attachment === 'attaching' || view.attachment === 'resynchronizing' ? undefined
    : view.attachmentIntent !== 'wanted' || !view.target ? { action: 'open', label: 'Open Session' }
    : view.attachment !== 'attached' ? { action: 'refresh', label: 'Retry connection' } : undefined;
  const uncertain = state.uncertain.some(item => sessionId !== undefined && item.sessionId === sessionId)
    || view?.cancellation?.status === 'uncertain' || view?.modelMutation?.status === 'uncertain'
    || view?.snapshot?.background?.some(tool => tool.state === 'outcome_unknown')
    || view?.snapshot?.attempt?.foreground?.some(tool => tool.state.type === 'settled' && tool.state.result.status.type === 'outcome_unknown')
    || view?.snapshot?.workflows?.runs.some(run => run.state.type === 'settled' && run.state.outcome === 'outcome_unknown');
  if (uncertain) return { status: 'uncertain', label: 'Needs verification', severity: 'warning', recovery,
    detail: `An operation may have taken effect. Check the conversation and affected work before trying again.${view?.snapshot?.durability_failure ? ' Storage also reported a failure; changes may not be saved.' : ''} Details are available in Developer Inspector.` };
  if (view?.snapshot?.durability_failure) return { status: 'failure', label: 'Changes may not be saved', severity: 'error', recovery,
    detail: 'Storage reported a failure. Resolve the storage problem before continuing; inspect the recorded details.' };
  if (state.connection === 'incompatible') return { status: 'failure', label: 'Connection version mismatch', severity: 'error', recovery,
    detail: 'Use matching Web and server versions. Review Connection settings before connecting again.' };
  if (recovery) return { status: 'reconnect', label: 'Connection interrupted', severity: 'warning', recovery,
    detail: recovery.action === 'open' ? 'Open this Session to continue. Your previous requests will not be sent again.' : 'Reconnect to see current work. Your previous requests will not be sent again.' };
  if (connecting || view?.attachment === 'attaching' || view?.attachment === 'resynchronizing') return { status: 'connecting', label: 'Connecting…', severity: 'quiet' };
  if (!view?.snapshot) return { status: 'idle', severity: 'quiet' };
  const snapshot = view.snapshot;
  if (snapshot.shutting_down) return { status: 'stopping', label: 'Session is closing…', severity: 'warning' };
  if (view.cancellation) return { status: 'stopping', label: 'Stopping…', severity: 'quiet' };
  if (view.modelMutation) return { status: 'waiting', label: 'Updating model…', severity: 'quiet' };
  if (snapshot.pending_interactions?.length) return { status: 'waiting', label: 'Waiting for your response', severity: 'quiet' };
  if (snapshot.attempt && snapshot.attempt.phase.type !== 'settled') return { status: 'working', label: 'Working…', severity: 'quiet' };
  if (snapshot.inbound.pending?.length || view.submissions?.length) return { status: 'queued', label: 'Queued', severity: 'quiet' };
  if (snapshot.attempt?.phase.type === 'settled') {
    const outcome = snapshot.attempt.phase.outcome.type;
    if (outcome === 'failed' || outcome === 'timed_out' || outcome === 'limit_exceeded') return {
      status: 'failure', label: outcome === 'timed_out' ? 'Work timed out' : outcome === 'limit_exceeded' ? 'Work reached its limit' : 'Work could not finish',
      severity: 'error', detail: 'Review the conversation and settings before starting again. Details are available in Developer Inspector.',
    };
  }
  return { status: 'idle', severity: 'quiet' };
}
