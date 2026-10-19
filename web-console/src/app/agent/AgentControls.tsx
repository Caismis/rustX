import { message } from '../../locale/translation';
import { useTranslation, useNotice } from '../../locale/react';
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Popover } from '@base-ui/react/popover';
import { sourceObservation } from '../../../../protocol/app-server/source-observation';
import type { SourceFieldPage, CheckpointPage, ModelCatalogView, SessionModelConfig, SourceSettings } from '../../../../protocol/app-server/v52';
import { AppServerClient, isOutcomeUncertain, sameTarget, type SessionView } from '../../client/app-server';
import type { ModelPickerState } from '../composer/ModelPicker';
import { ModelSelect } from '../../presentation/agent/ModelSelect';
import { Toast } from '../../presentation/primitives/Toast';
import { IconWarningOutline16 } from '../../presentation/primitives/icons';
import { Button } from '../../presentation/primitives/Button';
import { activeAttempt } from '../../bindings/projection';
import { catalogAdmits, catalogChoices, intentProfile, nextModelSelection, selectionChanges } from '../../bindings/model-catalog';
import type { ModelSelectionIntent } from '../../presentation/agent/ModelSelect';
import { selectSessionModel } from '../model-preference';

/** Catalog reads follow the attachment. While connecting, display client-owned
 * intent; after apply, native confirmation unlocks dependent sends. */
export function AgentControls({ client, view, draft, coldSource, blocked: pending = false, children }: { children?: (toolbar: ReactNode, picker: ModelPickerState) => ReactNode; client: AppServerClient; view?: SessionView; coldSource?: SourceSettings; blocked?: boolean; draft?: { source?: SourceSettings; intent?: SessionModelConfig; choose: (selection: SessionModelConfig) => void; disabled: boolean } }) {
  const tx = useTranslation();
 const [sourcePage,setSourcePage] = useState<SourceFieldPage>();
 const [sourceCatalog,setSourceCatalog] = useState<ModelCatalogView>();
 const [catalogPage,setCatalogPage] = useState<CheckpointPage>();
 const [catalog, setCatalog] = useState<ModelCatalogView>();
 const [busy, setBusy] = useState(false), [error, setError] = useNotice(), [blocked, setBlocked] = useState(true);
 const [selectionError, setSelectionError] = useNotice(), [toastSequence, setToastSequence] = useState(0);
 const root = useRef<HTMLDivElement>(null);
 const guard = useRef(false), epoch = useRef(0), sourceEpoch = useRef(0);
 const mounted = useRef(true), selection = useRef<{ scope: object } | undefined>(undefined);
 useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
 const generation = client.getSnapshot().generation;
 const target = view?.target;
 const scope = useMemo(() => ({}), [view?.id, generation, target?.session_id, target?.conversation_id, target?.runtime_incarnation, target?.attachment_id]);
 const currentScope = useRef(scope); currentScope.current = scope;
 const connecting = view?.attachment === 'attaching' && view.attachmentIntent === 'wanted' && !view.deleting && client.getSnapshot().connection === 'connected';
 const coldCatalog = connecting && coldSource?.session_models?.kind === 'available' ? coldSource.session_models : undefined;
 const coldError = connecting && coldSource?.session_models?.kind === 'unavailable' ? coldSource.session_models.diagnostic : undefined;
 const queued = view?.modelIntent;
 const admission = view?.attachmentObservation;
 const attached = !!view && client.isAttachmentControlCurrent(view.id, admission);
 const current = (at: number) => {
   const live = client.getSnapshot(), owned = live.views[view!.id];
   return mounted.current && currentScope.current === scope && epoch.current === at && live.generation === generation && sameTarget(owned?.target, target)
     && owned?.attachmentIntent === 'wanted' && !owned.deleting;
 };
 const read = async (at: number, previous?: CheckpointPage) => {
   if (!target || !current(at)) return;
     const result = await client.modelCatalogRead(target,()=>current(at),previous);
     if (!current(at)) return;
     const observed = client.getSnapshot().views[view!.id];
     if (observed?.modelMutation && observed.modelMutation.status !== 'in-flight') {
       await client.repairAgentModel(view!.id);
     }
     if (current(at)) { setCatalog(result.catalog); setCatalogPage(result.page); setError(''); }
   if (current(at)) setBlocked(false);
 };
 const load = (refresh = false) => {
   if (!attached || guard.current || !refresh && catalog && !blocked) return;
   const at = epoch.current; guard.current = true; setBusy(true); setBlocked(true);
   void read(at).catch(cause => { if (current(at)) setError(String(cause)); }).finally(() => { if (current(at)) { guard.current = false; setBusy(false); } });
 };
 useEffect(() => {
   ++epoch.current; setCatalog(undefined); setBlocked(true); setBusy(false); setError(''); setSelectionError(''); guard.current = false;
   if(!draft&&view?.observationState!=='checkpointLoading'&&view?.observationState!=='subscribePending')load(true);
   return () => { ++epoch.current; };
 }, [target?.attachment_id, generation, view?.snapshot?.resources?.revision, view?.snapshot?.model_status?.binding_revision]);
 useEffect(()=>{if(!draft&&view?.observationState==='following'&&!catalog&&blocked)load();},[view?.observationState]);
 const mutate = async (operation: () => Promise<unknown>) => {
   if (pending || guard.current || blocked || busy || !attached || selection.current?.scope === scope) return false;
   const owner = { scope }; selection.current = owner;
   // A native binding update invalidates catalog reads, not this acknowledged
   // gesture. Its original editor still decides whether its revision is consumed.
   const owns = () => {
     const live = client.getSnapshot(), owned = live.views[view!.id];
     return mounted.current && currentScope.current === scope && selection.current === owner && live.generation === generation
       && sameTarget(owned?.target, target) && owned?.attachmentIntent === 'wanted' && !owned.deleting;
   };
   const at = epoch.current; guard.current = true; setBusy(true); setBlocked(true); setError(''); setSelectionError('');
   try { await operation(); if (current(at)) await read(at); return owns(); }
   catch (cause) { if (current(at)) { const notice = isOutcomeUncertain(cause) ? message('agent:copy.outcome-uncertain-no-replay-reconnect-and-reread-authority-before-continuing') : message('agent:copy.value-reread-authority-before-continuing', { p0: String(cause) }); setError(notice); setSelectionError(notice); setToastSequence(sequence => sequence + 1); } }
   finally { if (selection.current === owner) selection.current = undefined; if (epoch.current === at) { guard.current = false; setBusy(false); } }
 };
 const model = view?.snapshot?.model;
 const modelStatus = view?.snapshot?.model_status;
 const frozen = activeAttempt(view?.snapshot) ? view?.snapshot?.attempt?.model : undefined;
 const sessionProfile = model?.configured.profile ?? (model?.unavailable == null ? model?.effective?.profile : undefined);
 const configured = (queued?.phase !== 'failed' ? queued?.config : undefined) ?? (model?.configured ?? (view?.settings ? view.settings.model ?? (coldSource?.session_models?.kind === 'available' ? coldSource.session_models.default_model : undefined) : undefined));
 const source = draft?.source ?? coldSource;
 const sourceChoices = draft?.source?.session_models?.kind === 'available' ? draft.source.session_models.catalog : coldCatalog?.catalog;
 const choices = sourceCatalog ?? (sourceChoices ? {models:(sourceChoices.models??[]).slice(0,32)} : catalog);
 const draftError = draft?.source?.session_models?.kind === 'unavailable' ? draft.source.session_models.diagnostic : undefined;
 // Reading choices locks selection through loading, not the menu trigger.
 const disabled = pending || !attached && !connecting || !!queued && queued.phase !== 'failed' && !connecting;
 // Configured intent (native, queued or draft) decides selections; the native
 // effective Profile is displayed only, and only for the configured Model.
 const selected = draft ? draft.intent : configured;
 const currentModel = selected?.model;
 const currentProfile = selected?.profile ?? undefined;
 // An unavailable selection has no effective Profile for new work.
 const effectiveProfile = !draft && attached && !queued && model?.unavailable == null && model?.effective?.model === currentModel ? model?.effective?.profile ?? undefined : undefined;
 const choose = async (intent: ModelSelectionIntent) => {
   if (!catalogAdmits(choices, intent.model, intentProfile(intent))) return false;
   // A partial primary gesture must carry the independently configured Summary
   // and overrides. Deferred native configuration is not an empty configuration.
   if (!draft && attached && modelStatus?.body_deferred && !selected) return false;
   // A Draft compares local intent. Existing Sessions, including a queued
   // choice during attachment, let native authority decide whether equal
   // values persist inherited intent or are already an explicit, durable NoOp.
   if (draft && !selectionChanges(selected?.model, selected?.profile ?? undefined, intent)) return true;
   const selection = nextModelSelection(selected, intent);
   if (draft) { draft.choose(selection); return true; }
   if (connecting) {
     void selectSessionModel(client, view!.id, selection).catch(() => {});
     return true;
   }
   return !!await mutate(() => selectSessionModel(client, view!.id, selection));
 };
 const sourceTotal = source && sourceObservation(source)?.collections.find(row=>row.field==='session_models')?.total;
 const sourceOffset = sourcePage?.next_offset ?? (sourcePage ? undefined : sourceTotal != null && Number(sourceTotal)>32 ? 32 : undefined);
 useEffect(()=>{++sourceEpoch.current;setSourcePage(undefined);setSourceCatalog(undefined);return()=>{++sourceEpoch.current;};},[source]);
 const nextSourcePage = source && sourceOffset != null ? async () => {
   if(guard.current)return;const at=epoch.current,sourceAt=sourceEpoch.current,owned=source;guard.current=true;setBusy(true);
   const currentSource=()=>epoch.current===at && sourceEpoch.current===sourceAt && client.getSnapshot().generation===generation;
   try {const result=await client.sourceModelCatalogRead(owned,currentSource,sourceOffset);if(currentSource()){setSourcePage(result.page);setSourceCatalog(result.catalog);}}
   catch(cause){if(currentSource())setError(String(cause));}
   finally{if(epoch.current===at){guard.current=false;setBusy(false);}}
 } : undefined;
 const nextPage = catalogPage?.next_offset != null && attached ? async () => {
   if(guard.current)return;guard.current=true;setBusy(true);const at=epoch.current;
   try {await read(at,catalogPage);}catch(cause){if(current(at))setError(String(cause));}finally{if(epoch.current===at){guard.current=false;setBusy(false);}}
 } : undefined;
 const picker: ModelPickerState = { nextPage: sourceChoices ? nextSourcePage : nextPage, choices: catalogChoices(choices), current: currentModel, profile: currentProfile, effectiveProfile,
   disabled: draft ? draft.disabled || !choices : disabled || (connecting ? !coldCatalog : blocked), loading: draft ? !draft.source : connecting ? !coldSource : busy && !catalog,
   error: draft ? draftError : queued?.error ?? coldError ?? (view?.modelMutation?.status === 'uncertain' ? tx('agent:copy.outcome-uncertain-no-replay-reconnect-and-reread-authority-before-continuing') : error), choose };
 const toolbar = <div ref={root} className="agent-control"><ModelSelect binding={JSON.stringify([generation, target?.attachment_id, draft?.source?.target, view?.snapshot?.resources?.revision, view?.snapshot?.model_status?.binding_revision])} choices={picker.choices}
   current={picker.current} profile={picker.profile} effectiveProfile={picker.effectiveProfile} disabled={draft ? draft.disabled || !choices : disabled} loading={draft ? !draft.source : connecting ? !coldSource : blocked} error={picker.error} load={load}
   choose={intent => { void choose(intent); }}/>
   {!draft && modelStatus?.body_deferred && <p role="status">{tx('common:activity.model-details-deferred')}</p>}
   {!draft && modelStatus?.available === false && <p role="status">{tx('common:activity.model-unavailable')}</p>}
   {frozen && <Popover.Root key={view?.snapshot?.attempt?.attempt_id}>
     <Popover.Trigger className="running-model-trigger" data-running-model>{tx('agent:agent-controls.running')}{' '}{frozen.primary.model}{frozen.primary.profile ? ` / ${frozen.primary.profile}` : ''}</Popover.Trigger>
     <Popover.Portal><Popover.Positioner side="top" align="end" sideOffset={36} collisionPadding={12} className="running-model-positioner">
       <Popover.Popup className="running-model-details" data-running-model-details aria-label={tx('agent:agent-controls.running')}>
         {model && <p data-session-model>{tx('agent:agent-controls.session-selection')}{' '}{model.configured.model}{sessionProfile ? ` / ${sessionProfile}` : ''}</p>}
         <Popover.Description>{tx('agent:agent-controls.frozen-invocation')}</Popover.Description><pre>{JSON.stringify(frozen, null, 2)}</pre>
       </Popover.Popup>
     </Popover.Positioner></Popover.Portal>
   </Popover.Root>}
   {!draft && !busy && (blocked && error || queued?.phase === 'failed') && <Button size="sm" disabled={!attached} onClick={() => load(true)}>{tx('agent:agent-controls.reread-models')}</Button>}
 </div>;
 return <>{children ? children(toolbar, picker) : toolbar}{draftError && <p role="alert">{draftError}</p>}{selectionError && <Toast key={toastSequence} text={selectionError} icon={<IconWarningOutline16 />} anchor={root.current?.closest<HTMLElement>('[data-composer-card]') ?? null} onDone={() => setSelectionError('')}/>}</>;
}
