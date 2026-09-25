import { useEffect, useRef, useState } from 'react';
import type { ModelCatalogView, SessionModelConfig, SourceSettings } from '../../../../protocol/app-server/v22';
import { AppServerClient, isOutcomeUncertain, sameTarget, type SessionView } from '../../client/app-server';
import { ModelSelect } from '../../presentation/agent/ModelSelect';
import { Button } from '../../presentation/primitives/Button';
import { activeAttempt } from '../../bindings/projection';
import { catalogAdmits, catalogChoices } from '../../bindings/model-catalog';
import { modelPreferences } from '../model-preference';

/** Replaceable read cache scoped to one native attachment. Mutations never update
 * displayed selection; only a subsequent authoritative read unlocks controls. */
export function AgentControls({ client, view, draft }: { client: AppServerClient; view?: SessionView; draft?: { source?: SourceSettings; intent?: SessionModelConfig; choose: (selection: SessionModelConfig) => void; disabled: boolean } }) {
 const [catalog, setCatalog] = useState<ModelCatalogView>();
 const [busy, setBusy] = useState(false), [error, setError] = useState(''), [blocked, setBlocked] = useState(true);
 const guard = useRef(false), epoch = useRef(0);
 const generation = client.getSnapshot().generation;
 const target = view?.target;
 const attached = view?.attachment === 'attached' && view?.attachmentIntent === 'wanted';
 const current = (at: number) => epoch.current === at && client.getSnapshot().generation === generation && sameTarget(client.getSnapshot().views[view!.id]?.target, target) && client.getSnapshot().views[view!.id]?.attachment === 'attached';
 const read = async (at: number) => {
   if (!target || !current(at)) return;
     const result = await client.request({ method: 'session/models', params: { target } }, 'models');
     await client.request({ method: 'session/model', params: { target } }, 'model');
     await client.repairAgentModel(view!.id);
     if (current(at)) setCatalog(result.catalog);
   if (current(at)) setBlocked(false);
 };
 const load = () => {
   if (!attached || guard.current) return;
   const at = epoch.current; guard.current = true; setBusy(true); setBlocked(true);
   void read(at).catch(cause => { if (current(at)) setError(String(cause)); }).finally(() => { if (current(at)) { guard.current = false; setBusy(false); } });
 };
 useEffect(() => {
   ++epoch.current; setCatalog(undefined); setBlocked(true); setBusy(false); setError(''); guard.current = false;
   return () => { ++epoch.current; };
 }, [target?.attachment_id, generation, attached, view?.snapshot?.resources?.revision]);
 const mutate = async (operation: () => Promise<unknown>) => {
   if (guard.current || blocked || busy || !attached) return;
   const at = epoch.current; guard.current = true; setBusy(true); setBlocked(true); setError('');
   try { await operation(); if (current(at)) await read(at); }
   catch (cause) { if (epoch.current === at) setError(isOutcomeUncertain(cause) ? 'Outcome uncertain. No replay; reconnect and reread authority before continuing.' : `${String(cause)} Reread authority before continuing.`); }
   finally { if (epoch.current === at) { guard.current = false; setBusy(false); } }
 };
 const model = view?.snapshot?.model;
 const choices = draft?.source?.session_models?.kind === 'available' ? draft.source.session_models.catalog : catalog;
 const draftError = draft?.source?.session_models?.kind === 'unavailable' ? draft.source.session_models.diagnostic : undefined;
 const disabled = !attached || busy;
 return <div className="agent-control"><ModelSelect binding={JSON.stringify([generation, target?.attachment_id, draft?.source?.target])} choices={catalogChoices(choices)}
   current={draft ? draft.intent?.model : model?.configured.model} profile={(draft ? draft.intent?.reasoningProfile : model?.effective.reasoningProfile) ?? undefined} disabled={draft ? draft.disabled || !choices : disabled} loading={draft ? !draft.source : blocked} error={draft ? draftError : error} load={draft ? () => {} : load}
   choose={(selected, profile) => { if (!catalogAdmits(choices, selected, profile)) return;
     const selection = { model: selected, ...(profile === undefined ? {} : { reasoningProfile: profile }) };
     if (draft) { draft.choose(selection); return; }
     const at = epoch.current;
     void mutate(async () => {
       await client.setAgentModel(view!.id, selection);
       if (current(at)) modelPreferences().select(client.getSnapshot().endpoint ?? '', selection);
     }); }}/>
   {activeAttempt(view?.snapshot) && view?.snapshot?.attempt?.model && view?.snapshot.attempt.model.primary.model !== model?.effective.model && <small>Running: {view?.snapshot?.attempt?.model?.primary.model}</small>}
   {!draft && blocked && !busy && error && <Button size="sm" disabled={!attached} onClick={load}>Reread models</Button>}
 </div>;
}
