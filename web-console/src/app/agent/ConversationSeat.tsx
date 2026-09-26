import { useTranslation } from '../../locale/react';
import { useState } from 'react';
import type { AppServerClient } from '../../client/app-server';
import { useClientSelector, sameValue, transportSelection } from '../../client/selectors';
import type { ProductHostWorkspaces } from '../../workspaces/host';
import type { UserInputBlock } from '../../../../protocol/app-server/v23';
import { activeAttempt, lineageSwitchSafe } from '../../bindings/projection';
import { goalDock } from '../../bindings/composer-context';
import { deriveSessionProductState, type SessionRecovery } from '../../bindings/session-product';
import { SessionStatus } from '../SessionStatus';
import { ConversationComposer } from '../new-conversation/ConversationComposer';
import { ConversationDocks, ConversationTotals } from './ConversationLive';
import { Interactions } from './Interactions';
import type { CommandId } from '../commands/registry';
import css from '../../presentation/agent/Conversation.module.css';

export function ConversationStatus({ client, sessionId, recover }: { client: AppServerClient; sessionId: string; recover: (action: SessionRecovery) => void }) {
  const tx = useTranslation();
  const product = useClientSelector(client, state => deriveSessionProductState(tx, state, state.views[sessionId]), sameValue);
  return <SessionStatus product={product} recover={recover}/>;
}

/** Execution admission belongs at the resident composer seat, never the shell. */
export function ConversationSeat({ client, host, sessionId, initialWorkspace, binding, current, consumed, restored, opened, onCommand }: {
  client: AppServerClient; host: ProductHostWorkspaces; sessionId?: string; initialWorkspace?: string;
  binding: string; current: () => boolean; consumed?: { id: string; sequence: number };
  restored?: { conversation: string; content: UserInputBlock[] }; opened: (id: string) => void;
  onCommand: (id: CommandId) => void;
}) {
  const state = useClientSelector(client, state => ({ ...transportSelection(state), composer: composerFacts(sessionId ? state.views[sessionId] : undefined) }), sameValue);
  const view = sessionId ? client.getSnapshot().views[sessionId] : undefined;
  const owner = JSON.stringify([state.generation, sessionId, binding]);
  const [sending, setSending] = useState<string>();
  const [failure, setFailure] = useState<{ owner: string; message: string }>();
  const error = failure?.owner === owner ? failure.message : undefined;
  const attached = state.connection === 'connected' && !view?.deleting && view?.attachmentIntent === 'wanted' && view.attachment === 'attached';
  const disabled = !attached || !!view?.modelMutation || !!view?.snapshot?.shutting_down || !!view?.snapshot?.durability_failure;
  return <div className={css.composerSeat} data-composer-seat="">
    <div hidden={!!view?.snapshot?.pending_interactions?.length}>
      <ConversationComposer client={client} host={host} initialWorkspace={initialWorkspace} binding={binding} activeView={view} current={current} consumed={consumed} opened={opened}
        context={view && <ConversationDocks key={view.id} client={client} sessionId={view.id} disabled={disabled}/>}
        active={view ? {
          initialContent: restored?.conversation === view.snapshot?.conversation_id ? restored?.content : undefined,
          submitDisabled: false, disabled, busy: sending === owner, active: activeAttempt(view.snapshot),
          lineageSwitchSafe: lineageSwitchSafe(view), hasGoal: !!goalDock(view.snapshot), onCommand,
          consumed, cancellationAvailable: attached && !view.cancellation && !view.snapshot?.shutting_down && !view.snapshot?.durability_failure,
          onCancel: () => { setFailure(undefined); void client.cancelTurn(view.id).catch(cause => setFailure({ owner, message: String(cause) })); },
          onUpload: files => client.upload(view.id, files),
          onSend: async (text, receipts, delivery) => {
            const generation = client.getSnapshot().generation; setFailure(undefined); setSending(owner);
            try { await client.send(view.id, text, receipts, delivery); return generation === client.getSnapshot().generation; }
            finally { setSending(value => value === owner ? undefined : value); }
          },
        } : undefined}/>
      {error && <p role="alert">{error}</p>}
      {view && <ConversationTotals client={client} sessionId={view.id}/>}
    </div>
    {sessionId && <LiveInteractions key={owner} client={client} sessionId={sessionId}/>}
  </div>;
}
function composerFacts(view: ReturnType<AppServerClient['getSnapshot']>['views'][string] | undefined) {
  if (!view) return undefined;
  return { id: view.id, target: view.target, attachment: view.attachment, attachmentIntent: view.attachmentIntent, deleting: view.deleting,
    modelMutation: view.modelMutation, cancellation: view.cancellation, active: activeAttempt(view.snapshot),
    resources: view.snapshot?.resources?.revision, attemptModel: view.snapshot?.attempt?.model,
    lineage: lineageSwitchSafe(view), goal: !!goalDock(view.snapshot), model: view.snapshot?.model,
    conversation: view.snapshot?.conversation_id, shuttingDown: view.snapshot?.shutting_down, durability: view.snapshot?.durability_failure,
    interactions: !!view.snapshot?.pending_interactions?.length };
}
function LiveInteractions({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const state = useClientSelector(client, state => state);
  const view = state.views[sessionId];
  return view && <Interactions client={client} state={state} view={view}/>;
}
