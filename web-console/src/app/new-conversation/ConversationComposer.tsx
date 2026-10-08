import startupCss from './FirstSubmission.module.css';
import { message, displayText, type Message } from '../../locale/translation';
import { useTranslation, useNotice } from '../../locale/react';
import { useClientSelector, transportSelection, sameValue } from '../../client/selectors';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted EmptyHero and WorkspacePicker; see PROVENANCE.md. */
import { useEffect, useState, useSyncExternalStore, type ComponentProps, type ReactNode } from 'react';
import type { SessionModelConfig } from '../../../../protocol/app-server/v38';
import type { AppServerClient, SessionView } from '../../client/app-server';
import { WorkspaceHostError, type ProductHostWorkspaces, type WorkspaceCatalog } from '../../workspaces/host';
import type { WorkspaceAuthority } from '../../workspaces/authority';
import type { WorkspaceAssociations, WorkspaceMutation } from '../../workspaces/associations';
import { sameEndpoint } from '../../workspaces/endpoint';
import { Menu } from '../../presentation/primitives/Menu';
import { Button } from '../../presentation/primitives/Button';
import { DialogSurface } from '../../presentation/primitives/DialogSurface';
import { IconChevronDownOutline14, IconFolderClose16 } from '../../presentation/primitives/icons';
import hero from '../../presentation/agent/HeroShell.module.css';
import modal from '../../presentation/primitives/Modal.module.css';
import { useModelPreference } from '../model-preference';
import { useAttachmentIntake } from '../composer/use-attachment-intake';
import { ComposerContextStack } from '../composer/ComposerContextStack';
import type { ModelPickerState } from '../composer/ModelPicker';
import { AgentControls } from '../agent/AgentControls';
import { AgentComposer } from '../agent/AgentComposer';
import { firstSubmitPort } from './port';
import { sessionModelBlock, WorkspaceControls, WorkspacePermission } from './WorkspaceControls';

export function ConversationComposer({ client, host, authority, associations, initialWorkspace, workspacePicked, current, opened, binding, active, activeView, context, consumed }: { authority: WorkspaceAuthority; associations: WorkspaceAssociations; activeView?: SessionView; binding: string; active?: ComponentProps<typeof AgentComposer>; context?: ReactNode; consumed?: { id: string; sequence: number }; client: AppServerClient; host: ProductHostWorkspaces; initialWorkspace?: string; workspacePicked?: (id: string) => void; current: () => boolean; opened: (id: string) => (() => boolean) | void }) {
  const tx = useTranslation();
  const [workspaceId, setWorkspace] = useState(initialWorkspace);
  const [owner, setOwner] = useState(binding);
  const [discarded, setDiscarded] = useState(0);
  const [intent, setIntent] = useState<SessionModelConfig>();
  const [catalog, setCatalog] = useState<WorkspaceCatalog>();
  const [error, setError] = useNotice(), [menu, setMenu] = useState(false), [add, setAdd] = useState<{ mutation: WorkspaceMutation; locations: { id: string; displayName: string }[] }>(), [adopting, setAdopting] = useState(false);
  // Transport transitions never remount this draft: each submission binds the
  // authority current at its own gesture, and a committed Session survives.
  const transport = useClientSelector(client, transportSelection, sameValue);
  const preference = useModelPreference(transport.endpoint ?? '');
  const submissions = client.firstSubmissions;
  const flow = useSyncExternalStore(submissions.subscribe, () => submissions.draft(binding) ?? (activeView ? submissions.session(activeView.id) : undefined));
  const drafting = !flow || flow.phase === 'rejected' || flow.phase === 'discarded';
  const pending = !!flow && !['admitted', 'discarded', 'rejected'].includes(flow.phase);
  useEffect(() => {
    if (owner === binding) return;
    setOwner(binding); setWorkspace(initialWorkspace); setIntent(undefined); setError('');
  }, [binding, owner, initialWorkspace]);
  const live = transport.connection === 'connected' && current();
  const connecting = live && activeView?.attachment === 'attaching' && activeView.attachmentIntent === 'wanted' && !activeView.deleting;
  useEffect(() => {
    let alive = true;
    void authority.observe().then(value => { if (alive && current() && value.current()) setCatalog(value.catalog); }, e => { if (alive && current()) setError(String(e)); });
    return () => { alive = false; };
  }, [authority, current, transport.endpoint, transport.authorityRevision]);
  const bound = sameEndpoint(catalog?.endpoint, transport.endpoint);
  const selected = bound ? catalog?.workspaces.find(w => w.id === workspaceId) : undefined;
  const pick = (id: string) => { setWorkspace(id); workspacePicked?.(id); setMenu(false); };
  const adopt = async (location: string) => {
    if (adopting || !add) return;
    const { mutation } = add;
    setAdopting(true); setError('');
    try {
      await host.adoptWorkspace(mutation.scope, location);
      if (!mutation.commit() || !current()) return;
      const next = await authority.observe();
      if (!current() || !mutation.current() || !next.current()) return;
      setCatalog(next.catalog); setAdd(undefined);
      const adopted = next.catalog.workspaces.find(w => w.location === location);
      if (adopted) pick(adopted.id);
    } catch (e) { if (current()) setError(String(e)); }
    finally { if (current()) setAdopting(false); }
  };
  const chooseModel = (selection: SessionModelConfig) => setIntent(selection);
  const intake = useAttachmentIntake(client.attachmentIntakes, JSON.stringify([client.getSnapshot().endpoint, client.getSnapshot().authorityId, binding, activeView?.id]), JSON.stringify([client.getSnapshot().generation, activeView?.target]));
  // The route owns a draft; acquiring its first native attachment must not
  // retire selected files. Native target changes still fence upload operations.
  const hasDraftFiles = useSyncExternalStore(intake.subscribe, intake.snapshot).some(file => file.status === 'draft');
  const composer = (block: () => Message | undefined, permission?: React.ReactNode, model?: React.ReactNode, modelPicker?: ModelPickerState) => <AgentComposer modelPicker={modelPicker} onRetainedRemove={flow && (flow.phase === 'paused' || ['failed', 'uncertain'].includes(flow.phase) && flow.failedPhase === 'uploading') ? id => submissions.removeUpload(flow, id) : undefined} onRetainedRecover={flow && ['failed', 'uncertain'].includes(flow.phase) ? retry => { void submissions.recoverUpload(flow, firstSubmitPort(client, host, current, opened), retry); } : undefined} uploadPolicy={client.getSnapshot().capabilities?.upload_policy} intakeOwner={intake} firstSubmission={flow} consumed={active ? consumed : undefined} commandAvailable={active ? id => id === 'model' && connecting || !active.disabled : id => id === 'model'} onCommand={() => {}} submitDisabled={!!block()} active={false}
    permission={permission} model={model} onCancel={() => {}} onUpload={async () => { throw new Error(tx('common:copy.no-session-exists-before-submit')); }} onSend={async () => false}
    onDraftSend={active && !connecting && !hasDraftFiles ? undefined : async (text, files, attachmentIds) => {
      if (activeView) return submissions.submit(binding, { workspaceId: '', text, files, attachmentIds }, firstSubmitPort(client, host, current, opened, true), () => intake.clear(), (files, ids) => intake.restoreDraft(files, ids), activeView.id);
      const blocked = block();
      if (blocked) { setError(blocked); return false; }
      if (!selected) { setError(message('common:copy.choose-a-host-authorized-registered-workspace-before-submitting')); return false; }
      return submissions.submit(binding, { workspaceId: selected.id, text, files, attachmentIds, model: intent ?? preference }, firstSubmitPort(client, host, current, opened), () => intake.clear(), (files, ids) => intake.restoreDraft(files, ids));
    }} {...active} busy={pending || !!active?.busy} disabled={active ? !pending && !connecting && active.disabled : !live || owner !== binding} binding={JSON.stringify([binding, discarded, flow?.phase === 'admitted' || flow?.phase === 'discarded'])}/>;
  return <div className={hero.body} data-resident-composer="" role="region" aria-label={active ? tx('common:conversation-composer.conversation-composer') : tx('common:conversation-composer.new-conversation')}>
    {!active && <div className={hero.headline}><h1 className={hero.titleGroup}>{tx('common:conversation-composer.what-would-you-like-to-build')}</h1></div>}
    {!active && <div className={hero.workspaceRow}>
      <Menu open={menu} autoFocus selectedId={selected?.id} onClose={() => setMenu(false)}
        items={[...(bound ? catalog?.workspaces ?? [] : []).map(w => ({ id: w.id, label: w.displayName, icon: <IconFolderClose16 size={16}/> })), ...(bound && catalog?.picker.kind === 'configured' ? [{ id: '::add-workspace', label: tx('common:conversation-composer.add-workspace') }] : [])]}
        onSelect={id => {
          if (id !== '::add-workspace') { pick(id); return; }
          setMenu(false);
          const mutation = associations.captureMutation(catalog);
          if (!mutation || catalog?.picker.kind !== 'configured') { setError(String(new WorkspaceHostError('Workspace Host authority replaced', 'authority_replaced'))); return; }
          setAdd({ mutation, locations: catalog.picker.locations });
        }}
        anchor={<button type="button" className={hero.workspace} aria-label={tx('common:conversation-composer.choose-workspace')} aria-haspopup="menu" aria-expanded={menu} disabled={!drafting || adopting} onClick={() => setMenu(v => !v)}><IconFolderClose16 size={16}/><span className={hero.workspaceLabel}>{selected?.displayName ?? tx('common:conversation-composer.choose-workspace')}</span><IconChevronDownOutline14 size={12}/></button>}/>
    </div>}
    <ComposerContextStack todo={context} goal={null} queue={null} composer={<WorkspaceControls client={client} host={host} workspaceId={active ? initialWorkspace : workspaceId}>{(source, approval) => { const selection = intent ?? preference ?? (source?.session_models?.kind === 'available' ? source.session_models.default_model : undefined); const block = () => approval.block() ?? sessionModelBlock(source, selection); return <><AgentControls client={client} view={activeView} coldSource={source} blocked={pending} draft={active ? undefined : { source, intent: selection, choose: chooseModel, disabled: !drafting }}>{(toolbar, picker) => composer(block, (active ? initialWorkspace : selected) && <WorkspacePermission source={source} approval={approval} disabled={active ? pending || !live : !drafting}/>, toolbar, picker)}</AgentControls>{!active && selected && block() && <small role="status">{displayText(tx, block()!)}</small>}</>; }}</WorkspaceControls>}/>
    {!active && catalog?.picker.kind === 'unavailable' && !selected && <small role="status">{catalog.picker.reason}</small>}
    {!active && !add && error && <p role="alert">{error}</p>}
    {flow && (flow.session || activeView) && pending && !['attaching', 'uploading', 'admitting'].includes(flow.phase) && <p className={startupCss.notice} role="status" data-first-submission={flow.phase}>{tx(flow.phase === 'paused' ? 'agent:upload.paused' : flow.phase === 'attaching' ? 'common:startup.attaching' : flow.phase === 'uploading' ? 'common:startup.uploading' : flow.phase === 'admitting' ? 'common:startup.admitting' : flow.phase === 'uncertain' ? 'common:startup.uncertain' : flow.failedPhase === 'attaching' ? 'common:startup.attach-failed' : flow.failedPhase === 'uploading' ? 'common:startup.upload-failed' : 'common:startup.admission-failed')}</p>}
    {flow?.error != null && <div className={startupCss.notice} role="alert">{activeView && !flow.session ? tx('common:startup.attach-failed') : !flow.session && flow.phase === 'rejected' ? tx('common:conversation-composer.no-session-was-created-correct-the-draft-or-workspace-and-submit') : flow.session ? null : tx('common:conversation-composer.creation-outcome-uncertain-inspect-native-sessions-before-starti')}<details><summary>{tx('common:app.show-details')}</summary><pre>{String(flow.error)}</pre>{flow.session && <p>{tx('common:conversation-composer.session-value-was-created-value-upload-s-committed-inspect-that', { p0: flow.session.id, p1: flow.receipts.length })}</p>}</details></div>}
    {flow?.phase === 'paused' && <Button onClick={() => void submissions.continueUploads(flow, firstSubmitPort(client, host, current, opened))}>{tx('agent:upload.continue')}</Button>}
    {flow && ['failed', 'uncertain'].includes(flow.phase) && <Button onClick={() => { setDiscarded(value => value + 1); submissions.discard(flow); }}>{tx('common:conversation-composer.discard-first-submission-draft')}</Button>}
    <DialogSurface open={!!add} onClose={() => { if (!adopting) setAdd(undefined); }} title={tx('common:conversation-composer.add-workspace-2')} overlayClassName={`${modal.root} ${modal.scrim}`} className={modal.dialog}>
      <div className={modal.content}><div className={modal.header}><h2 className={modal.title}>{tx('common:conversation-composer.add-workspace-2')}</h2></div><div className={modal.body}><p>{tx('common:conversation-composer.choose-a-location-authorized-by-this-product-host')}</p>{error && <p role="alert">{error}</p>}{add?.locations.map(location => <Button key={location.id} disabled={adopting} onClick={() => void adopt(location.id)}>{location.displayName}</Button>)}<Button disabled={adopting} onClick={() => setAdd(undefined)}>{tx('common:conversation-composer.cancel')}</Button></div></div>
    </DialogSurface>
  </div>;
}
