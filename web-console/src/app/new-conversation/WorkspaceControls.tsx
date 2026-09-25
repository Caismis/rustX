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
  const { actor } = useSettingsTarget(client, workspaceId ? workspaceSettingsTarget(workspaceId) : userSettingsTarget, host, !!workspaceId);
  const source = useSelector(actor, s => s.context.observation);
  const error = useSelector(actor, s => s.context.readError);
  return <SettingsActorContext value={actor}><SourceContext value={source}>{<ApprovalScope source={source}>{children}</ApprovalScope>}{error && <span role="alert">{error}<Button size="sm" onClick={() => actor.send({ type: 'REFRESH' })}>Reread Workspace</Button></span>}</SourceContext></SettingsActorContext>;
}
function useWorkspaceApproval(source?: SourceSettings) {
  const actor = useSettingsActor();
  const revision = source?.workspace?.revision ?? '';
  const unit = useUnitEditing<ApprovalMode>({ authored: source?.workspace?.authored?.approval_mode ?? undefined, blank: 'policy', revision, mutation: approvalMutation });
  return { actor, revision, unit, block: () => workspaceApprovalBlock(actor) };
}
type WorkspaceApproval = ReturnType<typeof useWorkspaceApproval>;
function ApprovalScope({ source, children }: { source?: SourceSettings; children: (source: SourceSettings | undefined, approval: WorkspaceApproval) => ReactNode }) {
  const approval = useWorkspaceApproval(source);
  return children(source, approval);
}
export function WorkspacePermission({ source, approval, disabled = false }: { source?: SourceSettings; approval: WorkspaceApproval; disabled?: boolean }) {
  const { actor, revision, unit } = approval;
  const mutation = approvalMutation;
  const blocked = disabled || !source?.prospective_approval_mode || !unit.admitted || unit.busy || unit.awaitingObservation || unit.reviewNeeded;
  const outcome = unit.outcome;
  return <div><PermissionSelect value={source?.prospective_approval_mode ?? undefined} disabled={blocked} choose={value => {
    unit.edit(value);
    actor.send({ type: 'UNIT.SUBMIT', identity: unit.identity, selector: revisionSelector(mutation(null)), revision, mutation: mutation(value) });
  }}/>
    {unit.draft && <small>Requested: {unit.displayed === 'full_access' ? 'Full access' : 'Policy'}</small>}
    {outcome.kind === 'conflict' && <small role="alert">Workspace changed. Your selection is preserved.</small>}
    {outcome.kind === 'rejected' && <small role="alert">{outcome.detail}</small>}
    {outcome.kind === 'uncertain' && <small role="alert">Outcome uncertain. Reread before another change; no replay.</small>}
    {(outcome.kind === 'committed' || outcome.kind === 'saved' && !outcome.observed) && <small role="status">Saved; awaiting authoritative observation.</small>}
    {unit.intent && !unit.busy && <Button size="sm" onClick={unit.discard}>Discard requested permission</Button>}
    {unit.reviewNeeded && <Button size="sm" onClick={unit.review}>Review current revision</Button>}
    {unit.draft && !blocked && <Button size="sm" onClick={() => unit.submit()}>Apply requested permission</Button>}
    {(unit.awaitingObservation || outcome.kind === 'uncertain' || outcome.kind === 'conflict') && <Button size="sm" onClick={() => actor.send({ type: 'REFRESH' })}>Reread permissions</Button>}
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
  if (source?.session_models?.kind === 'unavailable') return `Native cannot create a Session in this Workspace: ${source.session_models.diagnostic}`;
  return intent && source && !catalogAdmits(sessionCatalog(source), intent.model, intent.reasoningProfile ?? undefined)
    ? 'The selected model is not in this Workspace\'s native model catalog. Choose a model again.' : undefined;
}
