import type { ReactNode } from 'react';
import { useSelector } from '@xstate/react';
import type { ApprovalMode, SessionModelConfig, SourceSettings } from '../../../../protocol/app-server/v21';
import type { AppServerClient } from '../../client/app-server';
import type { ProductHostWorkspaces } from '../../workspaces/host';
import { ModelSelect } from '../../presentation/agent/ModelSelect';
import { PermissionSelect } from '../../presentation/agent/PermissionSelect';
import { Button } from '../../presentation/primitives/Button';
import { SettingsActorContext, useSettingsActor, useSettingsTarget } from '../settings/machines/react';
import { SourceContext } from '../settings/source-context';
import { workspaceSettingsTarget, userSettingsTarget, revisionSelector } from '../settings/projection';
import { useUnitEditing } from '../settings/forms/bridge';

/** Shared bounded configuration actor, independent of the Settings page tree. */
export function WorkspaceControls({ client, host, workspaceId, children }: { client: AppServerClient; host: ProductHostWorkspaces; workspaceId?: string; children: (source?: SourceSettings) => ReactNode }) {
  const { actor } = useSettingsTarget(client, workspaceId ? workspaceSettingsTarget(workspaceId) : userSettingsTarget, host, !!workspaceId);
  const source = useSelector(actor, s => s.context.observation);
  const error = useSelector(actor, s => s.context.readError);
  return <SettingsActorContext value={actor}><SourceContext value={source}>{children(source)}{error && <span role="alert">{error}<Button size="sm" onClick={() => actor.send({ type: 'REFRESH' })}>Reread Workspace</Button></span>}</SourceContext></SettingsActorContext>;
}
export function WorkspacePermission({ source, disabled = false }: { source?: SourceSettings; disabled?: boolean }) {
  const actor = useSettingsActor();
  const mutation = (authored: ApprovalMode | null) => ({ kind: 'config' as const, mutation: { unit: 'approval' as const, authored } });
  const revision = source?.workspace?.revision ?? '';
  const unit = useUnitEditing<ApprovalMode>({ authored: source?.workspace?.authored?.approval_mode ?? undefined, blank: 'policy', revision, mutation });
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
    {unit.reviewNeeded && <Button size="sm" onClick={unit.review}>Review current revision</Button>}
    {unit.draft && !blocked && <Button size="sm" onClick={() => unit.submit()}>Apply requested permission</Button>}
    {(unit.awaitingObservation || outcome.kind === 'uncertain' || outcome.kind === 'conflict') && <Button size="sm" onClick={() => actor.send({ type: 'REFRESH' })}>Reread permissions</Button>}
  </div>;
}
/** Current-file native model definitions are choices, never effective Session state. */
export function NewConversationModelControl({ source, intent, choose, disabled }: { source?: SourceSettings; intent?: SessionModelConfig; choose: (intent: SessionModelConfig) => void; disabled: boolean }) {
  return <ModelSelect choices={Object.entries(source?.resolved?.models ?? {}).map(([id, model]) => ({ id, profiles: Object.keys(model.reasoning?.profiles ?? {}).map(id => ({ id, label: id })), defaultProfile: model.reasoning?.default_profile }))}
    current={intent?.model} profile={intent?.reasoningProfile ?? undefined} disabled={disabled || !source?.resolved} loading={!source} load={() => {}}
    choose={(model, reasoningProfile) => choose({ model, ...(reasoningProfile === undefined ? {} : { reasoningProfile }) })}/>;
}
