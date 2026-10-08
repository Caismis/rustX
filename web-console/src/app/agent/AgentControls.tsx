import { message } from '../../locale/translation';
import { useTranslation, useNotice } from '../../locale/react';
import { useEffect, useRef, useState, type ReactNode } from 'react';
import type { ModelCatalogView, SessionModelConfig, SourceSettings } from '../../../../protocol/app-server/v38';
import { AppServerClient, isOutcomeUncertain, sameTarget, type SessionView } from '../../client/app-server';
import type { ModelPickerState } from '../composer/ModelPicker';
import { ModelSelect } from '../../presentation/agent/ModelSelect';
import { Button } from '../../presentation/primitives/Button';
import { activeAttempt } from '../../bindings/projection';
import { catalogAdmits, catalogChoices } from '../../bindings/model-catalog';
import { selectSessionModel } from '../model-preference';

/** Catalog reads follow the attachment. While connecting, display client-owned
 * intent; after apply, native confirmation unlocks dependent sends. */
export function AgentControls({ client, view, draft, coldSource, blocked: pending = false, children }: { children?: (toolbar: ReactNode, picker: ModelPickerState) => ReactNode; client: AppServerClient; view?: SessionView; coldSource?: SourceSettings; blocked?: boolean; draft?: { source?: SourceSettings; intent?: SessionModelConfig; choose: (selection: SessionModelConfig) => void; disabled: boolean } }) {
  const tx = useTranslation();
 const [catalog, setCatalog] = useState<ModelCatalogView>();
 const [busy, setBusy] = useState(false), [error, setError] = useNotice(), [blocked, setBlocked] = useState(true);
 const guard = useRef(false), epoch = useRef(0);
 const generation = client.getSnapshot().generation;
 const target = view?.target;
 const connecting = view?.attachment === 'attaching' && view.attachmentIntent === 'wanted' && !view.deleting && client.getSnapshot().connection === 'connected';
 const coldCatalog = connecting && coldSource?.session_models?.kind === 'available' ? coldSource.session_models : undefined;
 const coldError = connecting && coldSource?.session_models?.kind === 'unavailable' ? coldSource.session_models.diagnostic : undefined;
 const queued = view?.modelIntent;
 const attached = view?.attachment === 'attached' && view?.attachmentIntent === 'wanted';
 const current = (at: number) => epoch.current === at && client.getSnapshot().generation === generation && sameTarget(client.getSnapshot().views[view!.id]?.target, target) && client.getSnapshot().views[view!.id]?.attachment === 'attached';
 const read = async (at: number) => {
   if (!target || !current(at)) return;
     const result = await client.request({ method: 'session/models', params: { target } }, 'models');
     if (result.catalog.models?.length || view?.snapshot?.model) {
       await client.request({ method: 'session/model', params: { target } }, 'model');
       await client.repairAgentModel(view!.id);
     }
     if (current(at)) setCatalog(result.catalog);
   if (current(at)) setBlocked(false);
 };
 const load = (refresh = false) => {
   if (!attached || guard.current || !refresh && catalog && !blocked) return;
   const at = epoch.current; guard.current = true; setBusy(true); setBlocked(true);
   void read(at).catch(cause => { if (current(at)) setError(String(cause)); }).finally(() => { if (current(at)) { guard.current = false; setBusy(false); } });
 };
 useEffect(() => {
   ++epoch.current; setCatalog(undefined); setBlocked(true); setBusy(false); setError(''); guard.current = false;
   if (!draft) load(true);
   return () => { ++epoch.current; };
 }, [target?.attachment_id, generation, attached, view?.snapshot?.resources?.revision]);
 const mutate = async (operation: () => Promise<unknown>) => {
   if (pending || guard.current || blocked || busy || !attached) return false;
   const at = epoch.current; guard.current = true; setBusy(true); setBlocked(true); setError('');
   try { await operation(); if (current(at)) await read(at); return current(at); }
   catch (cause) { if (epoch.current === at) setError(isOutcomeUncertain(cause) ? message('agent:copy.outcome-uncertain-no-replay-reconnect-and-reread-authority-before-continuing') : message('agent:copy.value-reread-authority-before-continuing', { p0: String(cause) })); }
   finally { if (epoch.current === at) { guard.current = false; setBusy(false); } }
 };
 const model = view?.snapshot?.model;
 const configured = (queued?.phase !== 'failed' ? queued?.config : undefined) ?? (attached ? model?.configured : view?.settings ? view.settings.model ?? (coldSource?.session_models?.kind === 'available' ? coldSource.session_models.default_model : undefined) : model?.configured);
 const choices = draft?.source?.session_models?.kind === 'available' ? draft.source.session_models.catalog : coldCatalog?.catalog ?? catalog;
 const draftError = draft?.source?.session_models?.kind === 'unavailable' ? draft.source.session_models.diagnostic : undefined;
 // Reading choices locks selection through loading, not the menu trigger.
 const disabled = pending || !attached && !connecting || !!queued && queued.phase !== 'failed' && !connecting;
 const choose = async (selected: string, profile?: string) => {
   if (!catalogAdmits(choices, selected, profile)) return false;
   const selection = { model: selected, ...(profile === undefined ? {} : { reasoningProfile: profile }) };
   if (draft) { draft.choose(selection); return true; }
   if (connecting) {
     void selectSessionModel(client, view!.id, selection).catch(() => {});
     return true;
   }
   return !!await mutate(() => selectSessionModel(client, view!.id, selection));
 };
 const picker: ModelPickerState = { choices: catalogChoices(choices), current: draft ? draft.intent?.model : configured?.model,
   disabled: draft ? draft.disabled || !choices : disabled || (connecting ? !coldCatalog : blocked), loading: draft ? !draft.source : connecting ? !coldSource : busy && !catalog,
   error: draft ? draftError : queued?.error ?? coldError ?? error, choose };
 const toolbar = <div className="agent-control"><ModelSelect binding={JSON.stringify([generation, target?.attachment_id, draft?.source?.target, view?.snapshot?.resources?.revision])} choices={picker.choices}
   current={picker.current} profile={(draft ? draft.intent?.reasoningProfile : attached && !queued ? model?.effective.reasoningProfile : configured?.reasoningProfile) ?? undefined} disabled={draft ? draft.disabled || !choices : disabled} loading={draft ? !draft.source : connecting ? !coldSource : blocked} error={picker.error} load={() => load()}
   choose={(selected, profile) => { void choose(selected, profile); }}/>
   {activeAttempt(view?.snapshot) && view?.snapshot?.attempt?.model && view?.snapshot.attempt.model.primary.model !== model?.effective.model && <small>{tx('agent:agent-controls.running')}{' '}{view?.snapshot?.attempt?.model?.primary.model}</small>}
   {!draft && !busy && (blocked && error || queued?.phase === 'failed') && <Button size="sm" disabled={!attached} onClick={() => load(true)}>{tx('agent:agent-controls.reread-models')}</Button>}
 </div>;
 return children ? children(toolbar, picker) : toolbar;
}
