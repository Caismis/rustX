import { message } from '../../locale/translation';
import { useTranslation } from '../../locale/react';
import type { ReactNode } from 'react';
import { useSelector } from '@xstate/react';
import type { ApprovalMode, SessionModelConfig, SourceSettings } from '../../../../protocol/app-server/v23';
import type { AppServerClient } from '../../client/app-server';
import type { ProductHostWorkspaces } from '../../workspaces/host';
import { PermissionSelect } from '../../presentation/agent/PermissionSelect';
import { Button } from '../../presentation/primitives/Button';
import { SettingsActorContext, useSettingsActor, useSettingsTarget } from '../settings/machines/react';
import { SourceContext } from '../settings/source-context';
import { workspaceSettingsTarget, userSettingsTarget, revisionSelector } from '../settings/projection';
import { useUnitEditing } from '../settings/forms/bridge';
import { catalogAdmits } from '../../bindings/model-catalog';
import { approvalMutation, workspaceApprovalBlock } from './approval';

/** Shared bounded configuration actor, independent of the Settings page tree. */
export function WorkspaceControls({ client, host, workspaceId, children }: { client: AppServerClient; host: ProductHostWorkspaces; workspaceId?: string; children: (source: SourceSettings | undefined, approval: WorkspaceApproval) => ReactNode }) {
  const tx = useTranslation();
  const { actor } = useSettingsTarget(client, workspaceId ? workspaceSettingsTarget(workspaceId) : userSettingsTarget, host, !!workspaceId);
  const source = useSelector(actor, s => s.context.observation);
  const error = useSelector(actor, s => s.context.readError);
  return <SettingsActorContext value={actor}><SourceContext value={source}>{<ApprovalScope source={source}>{children}</ApprovalScope>}{error && <span role="alert">{error}<Button size="sm" onClick={() => actor.send({ type: 'REFRESH' })}>{tx('workspace:workspace-controls.reread-workspace')}</Button></span>}</SourceContext></SettingsActorContext>;
}
function useWorkspaceApproval(source?: SourceSettings) {
  const actor = useSettingsActor();
  const revision = source?.workspace?.revision ?? '';
  const unit = useUnitEditing<ApprovalMode>({ authored: source?.workspace?.authored?.approval_mode ?? undefined, blank: 'policy', revision, mutation: approvalMutation });
  return { actor, revision, unit, block: () => { const reason = workspaceApprovalBlock(actor); return reason && message(reason); } };
}
type WorkspaceApproval = ReturnType<typeof useWorkspaceApproval>;
function ApprovalScope({ source, children }: { source?: SourceSettings; children: (source: SourceSettings | undefined, approval: WorkspaceApproval) => ReactNode }) {
  const approval = useWorkspaceApproval(source);
  return children(source, approval);
}
export function WorkspacePermission({ source, approval, disabled = false }: { source?: SourceSettings; approval: WorkspaceApproval; disabled?: boolean }) {
  const tx = useTranslation();
  const { actor, revision, unit } = approval;
  const mutation = approvalMutation;
  const blocked = disabled || !source?.prospective_approval_mode || !unit.admitted || unit.busy || unit.awaitingObservation || unit.reviewNeeded;
  const outcome = unit.outcome;
  return <div><PermissionSelect value={source?.prospective_approval_mode ?? undefined} disabled={blocked} choose={value => {
    unit.edit(value);
    actor.send({ type: 'UNIT.SUBMIT', identity: unit.identity, selector: revisionSelector(mutation(null)), revision, mutation: mutation(value) });
  }}/>
    {unit.draft && <small>{tx('workspace:workspace-controls.requested')}{' '}{unit.displayed === 'full_access' ? tx('workspace:workspace-controls.full-access') : tx('workspace:workspace-controls.policy')}</small>}
    {outcome.kind === 'conflict' && <small role="alert">{tx('workspace:workspace-controls.workspace-changed-your-selection-is-preserved')}</small>}
    {outcome.kind === 'rejected' && <small role="alert">{outcome.detail}</small>}
    {outcome.kind === 'uncertain' && <small role="alert">{tx('workspace:workspace-controls.outcome-uncertain-reread-before-another-change-no-replay')}</small>}
    {(outcome.kind === 'committed' || outcome.kind === 'saved' && !outcome.observed) && <small role="status">{tx('workspace:workspace-controls.saved-awaiting-authoritative-observation')}</small>}
    {unit.intent && !unit.busy && <Button size="sm" onClick={unit.discard}>{tx('workspace:workspace-controls.discard-requested-permission')}</Button>}
    {unit.reviewNeeded && <Button size="sm" onClick={unit.review}>{tx('workspace:workspace-controls.review-current-revision')}</Button>}
    {unit.draft && !blocked && <Button size="sm" onClick={() => unit.submit()}>{tx('workspace:workspace-controls.apply-requested-permission')}</Button>}
    {(unit.awaitingObservation || outcome.kind === 'uncertain' || outcome.kind === 'conflict') && <Button size="sm" onClick={() => actor.send({ type: 'REFRESH' })}>{tx('workspace:workspace-controls.reread-permissions')}</Button>}
  </div>;
}
/** The native catalog a Session created in this Workspace binds. Its choices
 * are draft Session intent, never Workspace configuration or Session state. */
export function sessionCatalog(source?: SourceSettings) {
  return source?.session_models?.kind === 'available' ? source.session_models.catalog : undefined;
}
/** Native says a Session cannot be created here, or no longer publishes the
 * draft model intent. Either way nothing is submitted. */
export function sessionModelBlock(source: SourceSettings | undefined, intent?: SessionModelConfig) {
  if (source?.session_models?.kind === 'unavailable') return message('workspace:copy.native-cannot-create-a-session-in-this-workspace-value', { p0: source.session_models.diagnostic });
  return intent && source && !catalogAdmits(sessionCatalog(source), intent.model, intent.reasoningProfile ?? undefined)
    ? message('workspace:copy.the-selected-model-is-not-in-this-workspace-s-native-model-catalog-choose-a-model-again') : undefined;
}
