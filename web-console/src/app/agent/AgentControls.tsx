import { useEffect, useRef, useState } from 'react';
import type { ModelCatalogView } from '../../../../protocol/app-server/v21';
import { AppServerClient, isOutcomeUncertain, sameTarget, type SessionView } from '../../client/app-server';
import { ModelSelect } from '../../presentation/agent/ModelSelect';
import { Button } from '../../presentation/primitives/Button';
import { activeAttempt } from '../../bindings/projection';
import { catalogAdmits, catalogChoices } from '../../bindings/model-catalog';

/** Replaceable read cache scoped to one native attachment. Mutations never update
 * displayed selection; only a subsequent authoritative read unlocks controls. */
export function AgentControls({ client, view }: { client: AppServerClient; view: SessionView }) {
 const [catalog, setCatalog] = useState<ModelCatalogView>();
 const [busy, setBusy] = useState(false), [error, setError] = useState(''), [blocked, setBlocked] = useState(true);
 const guard = useRef(false), epoch = useRef(0);
 const generation = client.getSnapshot().generation;
 const target = view.target;
 const attached = view.attachment === 'attached' && view.attachmentIntent === 'wanted';
 const current = (at: number) => epoch.current === at && client.getSnapshot().generation === generation && sameTarget(client.getSnapshot().views[view.id]?.target, target) && client.getSnapshot().views[view.id]?.attachment === 'attached';
 const read = async (at: number) => {
   if (!target || !current(at)) return;
     const result = await client.request({ method: 'session/models', params: { target } }, 'models');
     await client.request({ method: 'session/model', params: { target } }, 'model');
     await client.repairAgentModel(view.id);
     if (current(at)) setCatalog(result.catalog);
   if (current(at)) setBlocked(false);
 };
 const load = () => {
   if (!attached || guard.current) return;
   const at = epoch.current; guard.current = true; setBusy(true); setBlocked(true);
   void read(at).catch(cause => { if (current(at)) setError(String(cause)); }).finally(() => { if (current(at)) { guard.current = false; setBusy(false); } });
 };
 useEffect(() => {
   ++epoch.current; setCatalog(undefined); setBlocked(true); setBusy(false); guard.current = false;
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
 return <div className="agent-control"><ModelSelect key={`${generation}:${target?.attachment_id}`} choices={catalogChoices(catalog)}
   current={model?.configured.model} profile={model?.effective.reasoningProfile ?? undefined} disabled={disabled} loading={blocked} error={error} load={load}
   choose={(selected, profile) => { if (!catalogAdmits(catalog, selected, profile)) return;
     void mutate(() => client.setAgentModel(view.id, { model: selected, ...(profile === undefined ? {} : { reasoningProfile: profile }) })); }}/>
   {activeAttempt(view.snapshot) && view.snapshot?.attempt?.model && view.snapshot.attempt.model.primary.model !== model?.effective.model && <small>Running: {view.snapshot?.attempt?.model?.primary.model}</small>}
   {blocked && !busy && error && <Button size="sm" disabled={!attached} onClick={load}>Reread models</Button>}
 </div>;
}
