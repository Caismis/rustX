import { useEffect, useRef, useState } from 'react';
import type { ModelCatalogView, SourceSettings } from '../../../../protocol/app-server/v7';
import { AppServerClient, isOutcomeUncertain, sameTarget, type SessionView } from '../../client/app-server';
import { ModelSelect } from '../../presentation/agent/ModelSelect';
import { PermissionSelect } from '../../presentation/agent/PermissionSelect';
import { Button } from '../../presentation/primitives/Button';
import { activeAttempt } from '../../bindings/projection';

/** Replaceable read cache scoped to one native attachment. Mutations never update
 * displayed selection; only a subsequent authoritative read unlocks controls. */
export function AgentControls({ client, view, kind }: { client: AppServerClient; view: SessionView; kind: 'model' | 'permission' }) {
 const [catalog, setCatalog] = useState<ModelCatalogView>();
 const [source, setSource] = useState<SourceSettings>();
 const [busy, setBusy] = useState(false), [error, setError] = useState(''), [blocked, setBlocked] = useState(true);
 const guard = useRef(false), epoch = useRef(0);
 const generation = client.getSnapshot().generation;
 const target = view.target;
 const attached = view.attachment === 'attached' && view.attachmentIntent === 'wanted';
 const current = (at: number) => epoch.current === at && client.getSnapshot().generation === generation && sameTarget(client.getSnapshot().views[view.id]?.target, target) && client.getSnapshot().views[view.id]?.attachment === 'attached';
 const read = async (at: number) => {
   if (!target || !current(at)) return;
   if (kind === 'model') {
     const result = await client.request({ method: 'settings/models', params: { target } }, 'models');
     await client.request({ method: 'settings/model', params: { target } }, 'model');
     await client.repairAgentModel(view.id);
     if (current(at)) setCatalog(result.catalog);
   } else {
     const result = await client.request({ method: 'configuration/sourcesRead', params: { session_id: view.id } }, 'source_settings');
     await client.refresh(view.id);
     if (current(at)) setSource(result.projection);
   }
   if (current(at)) setBlocked(false);
 };
 const load = () => {
   if (!attached || guard.current) return;
   const at = epoch.current; guard.current = true; setBusy(true); setBlocked(true);
   void read(at).catch(cause => { if (current(at)) setError(String(cause)); }).finally(() => { if (current(at)) { guard.current = false; setBusy(false); } });
 };
 useEffect(() => {
   ++epoch.current; setCatalog(undefined); setSource(undefined); setBlocked(true); setBusy(false); guard.current = false;
   return () => { ++epoch.current; };
 }, [target?.attachment_id, generation, attached, view.snapshot?.resources?.revision]);
 const mutate = async (operation: () => Promise<unknown>) => {
   if (guard.current || blocked || busy || !attached) return;
   const at = epoch.current; guard.current = true; setBusy(true); setBlocked(true); setError('');
   try { await operation(); if (current(at)) await read(at); }
   catch (cause) { if (epoch.current === at) setError(isOutcomeUncertain(cause) ? 'Outcome uncertain. No replay; reconnect and reread authority before continuing.' : `${String(cause)} Reread authority before continuing.`); }
   finally { if (epoch.current === at) { guard.current = false; setBusy(false); } }
 };
 const model = view.snapshot?.model;
 const disabled = !attached || busy;
 if (kind === 'model') return <div className="agent-control"><ModelSelect key={`${generation}:${target?.attachment_id}`} choices={(catalog?.models ?? []).map(value => ({ id: value.model, profiles: (value.reasoningProfiles ?? []).map(profile => ({ id: profile.id, label: profile.id })), defaultProfile: value.defaultReasoningProfile ?? undefined }))}
   current={model?.configured.model} profile={model?.effective.reasoningProfile ?? undefined} disabled={disabled} loading={blocked} error={error} load={load}
   choose={(selected, profile) => { if (!catalog?.models?.some(model => model.model === selected && (profile === undefined || model.reasoningProfiles?.some(item => item.id === profile)))) return;
     void mutate(() => client.setAgentModel(view.id, { model: selected, ...(profile === undefined ? {} : { reasoningProfile: profile }) })); }}/>
   {activeAttempt(view.snapshot) && view.snapshot?.attempt?.model && view.snapshot.attempt.model.primary.model !== model?.effective.model && <small>Running: {view.snapshot?.attempt?.model?.primary.model}</small>}
   {blocked && !busy && error && <Button size="sm" disabled={!attached} onClick={load}>Reread models</Button>}
 </div>;
 const desired = source?.prospective_approval_mode ?? undefined;
 const effective = activeAttempt(view.snapshot) ? view.snapshot?.attempt?.execution_settings?.approval_mode : view.snapshot?.effective_approval_mode;
 const effectiveLabel = `Effective${activeAttempt(view.snapshot) ? ' for running attempt' : ''}: ${effective ?? 'unavailable'}`;
 return <div className="agent-control"><PermissionSelect title={effectiveLabel} key={`${generation}:${target?.attachment_id}`} choices={[{ id: 'policy', label: 'Tool policy' }, { id: 'full_access', label: 'Full access' }]} desired={desired} disabled={disabled} loading={blocked} load={load}
 choose={mode => { if (mode !== 'policy' && mode !== 'full_access') return; void mutate(() => client.request({ method: 'configuration/sourceWrite', params: { session_id: view.id, expected_revision: source!.workspace.revision, mutation: { kind: 'config', scope: 'workspace', mutation: { unit: 'approval', authored: mode } } } }, 'source_settings')); }}/>
 {source?.loaded?.pending_reload && <><small>{effectiveLabel}</small><small>Desired: {desired ?? 'unavailable'} · pending Reload</small><Button size="sm" disabled={disabled || blocked || activeAttempt(view.snapshot)} onClick={() => void mutate(() => client.request({ method: 'configuration/reload', params: { target: target! } }, 'configuration_reloaded'))}>Apply saved policy</Button></>}
 {error && <p role="alert">{error}</p>}{blocked && !busy && error && <Button size="sm" disabled={!attached} onClick={load}>Reread policy</Button>}
 </div>;
}
