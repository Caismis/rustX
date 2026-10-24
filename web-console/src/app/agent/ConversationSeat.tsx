import { useTranslation } from '../../locale/react';
import { useState } from 'react';
import { UploadFailure } from '../../client/uploads';
import type { AppServerClient } from '../../client/app-server';
import { sameTarget } from '../../../../protocol/app-server/attachment-identity';
import { useClientSelector, sameValue, transportSelection } from '../../client/selectors';
import type { ProductHostWorkspaces } from '../../workspaces/host';
import type { WorkspaceAuthority } from '../../workspaces/authority';
import type { WorkspaceAssociations } from '../../workspaces/associations';
import type { UserInputBlock } from '../../../../protocol/app-server/v57';
import { activeAttempt, lineageSwitchSafe } from '../../bindings/projection';
import { goalDock } from '../../bindings/composer-context';
import { deriveSessionProductState, type SessionRecovery } from '../../bindings/session-product';
import { SessionStatus } from '../SessionStatus';
import { ConversationComposer } from '../new-conversation/ConversationComposer';
import { ConversationDocks, ConversationTotals } from './ConversationLive';
import { Interactions } from './Interactions';
import type { CommandId } from '../commands/registry';
import type { ComposerDraft } from '../composer/draft';
import { ComposerSeat } from './ComposerSeat';

export function ConversationStatus({ client, sessionId, recover }: { client: AppServerClient; sessionId: string; recover: (action: SessionRecovery) => void }) {
  const tx = useTranslation();
  const product = useClientSelector(client, state => deriveSessionProductState(tx, state, state.views[sessionId]), sameValue);
  return <SessionStatus product={product} recover={recover}/>;
}

/** Execution admission belongs at the resident composer seat, never the shell. */
export function ConversationSeat({ client, host, authority, associations, sessionId, initialWorkspace, workspacePicked, draftOwner, binding, current, executable, transitioning = false, consumed, restored, preparePublication, onCommand }: {
  client: AppServerClient; host: ProductHostWorkspaces; authority: WorkspaceAuthority; associations: WorkspaceAssociations; sessionId?: string; initialWorkspace?: string;
  workspacePicked?: (id: string) => void;
  draftOwner?: ComposerDraft; executable: boolean; transitioning?: boolean; binding: string; current: () => boolean; consumed?: { id: string; sequence: number };
  restored?: { conversation: string; content: UserInputBlock[] }; preparePublication: () => ((id: string) => (() => boolean) | void);
  onCommand: (id: CommandId) => void;
}) {
  const state = useClientSelector(client, state => ({ ...transportSelection(state), composer: composerFacts(sessionId ? state.views[sessionId] : undefined) }), sameValue);
  const view = sessionId ? client.getSnapshot().views[sessionId] : undefined;
  const owner = JSON.stringify([state.generation, sessionId, binding]);
  const nativeUploadCurrent = () => !!view && client.isAttachmentControlCurrent(view.id, view.attachmentObservation) && state.generation === client.getSnapshot().generation && sameTarget(view?.target, sessionId ? client.getSnapshot().views[sessionId]?.target : undefined);
  const uploadCurrent = () => current() && nativeUploadCurrent();
  const cancellation = view && executable && current() ? client.cancellationTarget(view.id) : undefined;
  const [sending, setSending] = useState<string>();
  const [failure, setFailure] = useState<{ owner: string; message: string }>();
  const error = failure?.owner === owner ? failure.message : undefined;
  const attached = state.connection === 'connected' && !!view && client.isAttachmentControlCurrent(view.id, view.attachmentObservation);
  const disabled = transitioning || !attached || !executable || !!view?.modelMutation || !!view?.snapshot?.shutting_down || !!view?.snapshot?.durability_failure;
  return <ComposerSeat>
    <div hidden={view?.observationState === 'following' && !!view.snapshot?.pending_interactions?.length}>
      <ConversationComposer draftOwner={draftOwner} client={client} host={host} authority={authority} associations={associations} initialWorkspace={initialWorkspace} workspacePicked={workspacePicked} binding={binding} activeView={view} current={current} consumed={consumed} preparePublication={preparePublication}
        context={view && <ConversationDocks key={view.id} client={client} sessionId={view.id} disabled={disabled}/>}
        active={view ? {
          initialContent: restored?.conversation === (view.nodeConversationId ?? view.target?.conversation_id ?? view.snapshot?.conversation_id) ? restored?.content : undefined,
          submissionTarget: view.target, submitDisabled: false, disabled, busy: sending === owner, active: activeAttempt(view.snapshot),
          lineageSwitchSafe: lineageSwitchSafe(view), hasGoal: !!goalDock(view.snapshot), onCommand,
          consumed, cancellationAvailable: !!cancellation, cancellationOnly: !attached && !!cancellation,
          cancellationScope: cancellation ? { authority: client, identity: JSON.stringify([binding, cancellation]) } : undefined,
          onCancel: () => { setFailure(undefined); if (!cancellation || !current()) return; void client.cancelTurn(cancellation).catch(cause => setFailure({ owner, message: String(cause) })); },
          onUpload: (files, operation) => {
            if (!uploadCurrent()) return Promise.reject(new UploadFailure('failed', 'Upload view replaced'));
            // The current gesture admits work. Once sent, its native result
            // belongs to this draft/attachment, independently of route rendering.
            return client.upload(view.id, files, { current: nativeUploadCurrent, acknowledged: () => {} }, operation);
          },
          onReconcile: async operation => { if (!uploadCurrent()) throw new Error('Upload view replaced'); const result = await client.uploadStatus(view.id, operation); if (!nativeUploadCurrent()) throw new Error('Upload view replaced'); return result; },
          onSend: async (text, receipts, delivery, acknowledged) => {
            if (!uploadCurrent()) return false;
            setFailure(undefined); setSending(owner);
            try {
              await client.send(view.id, text, receipts, delivery, acceptance => {
                const native = client.getSnapshot().views[view.id];
                const conversation = native?.nodeConversationId ?? native?.target?.conversation_id ?? native?.snapshot?.conversation_id;
                // This native callback survives history unmounting. Its evidence
                // accompanies settlement; it never authorizes another route's draft.
                acknowledged?.(acceptance, conversation ? { session_id: view.id, conversation_id: conversation } : undefined);
              }, uploadCurrent); return true;
            }
            finally { setSending(value => value === owner ? undefined : value); }
          },
        } : undefined}/>
      {error && <p role="alert">{error}</p>}
      {view && <ConversationTotals client={client} sessionId={view.id}/>}
    </div>
    {sessionId && <LiveInteractions key={owner} client={client} sessionId={sessionId}/>}
  </ComposerSeat>;
}
function composerFacts(view: ReturnType<AppServerClient['getSnapshot']>['views'][string] | undefined) {
  if (!view) return undefined;
  return { id: view.id, target: view.target, attachmentObservation: view.attachmentObservation, attachment: view.attachment, attachmentIntent: view.attachmentIntent, deleting: view.deleting,
    modelMutation: view.modelMutation, modelIntent: view.modelIntent, cancellation: view.cancellation, observationState: view.observationState, active: activeAttempt(view.snapshot), attemptId: view.snapshot?.attempt?.attempt_id,
    resources: view.snapshot?.resources?.revision, attemptModel: view.snapshot?.attempt?.model,
    lineage: lineageSwitchSafe(view), goal: !!goalDock(view.snapshot), model: view.snapshot?.model, modelStatus: view.snapshot?.model_status, settings: view.settings,
    conversation: view.snapshot?.conversation_id, shuttingDown: view.snapshot?.shutting_down, durability: view.snapshot?.durability_failure,
    interactions: !!view.snapshot?.pending_interactions?.length };
}
function LiveInteractions({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const state = useClientSelector(client, state => state);
  const view = state.views[sessionId];
  return view && <Interactions client={client} state={state} view={view}/>;
}
