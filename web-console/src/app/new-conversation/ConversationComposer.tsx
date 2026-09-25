import { useClientSelector, transportSelection, sameValue } from '../../client/selectors';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted EmptyHero and WorkspacePicker; see PROVENANCE.md. */
import { useEffect, useState, useRef, type ComponentProps, type ReactNode } from 'react';
import { useActorRef, useSelector } from '@xstate/react';
import type { SessionModelConfig } from '../../../../protocol/app-server/v23';
import type { AppServerClient, SessionView } from '../../client/app-server';
import type { ProductHostWorkspaces, WorkspaceCatalog } from '../../workspaces/host';
import { sameEndpoint } from '../../workspaces/endpoint';
import { Menu } from '../../presentation/primitives/Menu';
import { Button } from '../../presentation/primitives/Button';
import { DialogSurface } from '../../presentation/primitives/DialogSurface';
import { IconChevronDownOutline14, IconFolderClose16 } from '../../presentation/primitives/icons';
import hero from '../../presentation/agent/HeroShell.module.css';
import modal from '../../presentation/primitives/Modal.module.css';
import { useModelPreference } from '../model-preference';
import { ComposerContextStack } from '../composer/ComposerContextStack';
import { AgentControls } from '../agent/AgentControls';
import { AgentComposer } from '../agent/AgentComposer';
import { firstSubmitMachine } from './first-submit';
import { firstSubmitPort } from './port';
import { sessionModelBlock, WorkspaceControls, WorkspacePermission } from './WorkspaceControls';

export function ConversationComposer({ client, host, initialWorkspace, current, opened, binding, active, activeView, context, consumed }: { activeView?: SessionView; binding: string; active?: ComponentProps<typeof AgentComposer>; context?: ReactNode; consumed?: { id: string; sequence: number }; client: AppServerClient; host: ProductHostWorkspaces; initialWorkspace?: string; current: () => boolean; opened: (id: string, error?: string) => void }) {
  const [workspaceId, setWorkspace] = useState(initialWorkspace);
  const [owner, setOwner] = useState(binding);
  const [discarded, setDiscarded] = useState(0);
  const seat = useRef<HTMLDivElement>(null);
  const modelCommand = useRef(false);
  const [draftConsumed, setDraftConsumed] = useState<{ id: string; sequence: number }>();
  const complete = useRef<((accepted: boolean) => void) | undefined>(undefined);
  const notified = useRef(false);
  const [intent, setIntent] = useState<SessionModelConfig>();
  const [catalog, setCatalog] = useState<WorkspaceCatalog>();
  const [error, setError] = useState(''), [menu, setMenu] = useState(false), [add, setAdd] = useState(false), [adopting, setAdopting] = useState(false);
  // Transport transitions never remount this draft: each submission binds the
  // authority current at its own gesture, and a committed Session survives.
  const transport = useClientSelector(client, transportSelection, sameValue);
  const preference = useModelPreference(transport.endpoint ?? '');
  const actor = useActorRef(firstSubmitMachine);
  const flow = useSelector(actor, s => s);
  const drafting = flow.matches('drafting');
  useEffect(() => {
    if (owner === binding) return;
    complete.current?.(false); complete.current = undefined;
    notified.current = false;
    actor.send({ type: 'RESET' });
    setOwner(binding); setWorkspace(initialWorkspace); setIntent(undefined); setError('');
  }, [binding, owner, initialWorkspace, actor]);
  const live = transport.connection === 'connected' && current();
  useEffect(() => {
    let alive = true;
    void host.listWorkspaces().then(value => { if (alive && current()) setCatalog(value); }, e => { if (alive && current()) setError(String(e)); });
    return () => { alive = false; };
  }, [host, current, transport.endpoint, transport.authorityRevision]);
  useEffect(() => {
    // A FirstSubmitPort owns only the native continuation it started.  Once
    // create acknowledges, this exact Session is a committed native fact;
    // recovery belongs to the current New Conversation navigation epoch.
    if (!current() || notified.current) return;
    if (flow.matches('session')) {
      notified.current = true; complete.current?.(true); complete.current = undefined;
      opened(flow.context.session!.id);
    }
    // A later failure never rewinds creation: the committed Session is opened.
    else if (flow.matches('failed') && flow.context.session) {
      notified.current = true; complete.current?.(false); complete.current = undefined;
      opened(flow.context.session.id, `Session created. First submission stopped: ${String(flow.context.error)}. ${flow.context.receipts.length} upload(s) committed. No operation was replayed.`);
    } else if (flow.context.error) { complete.current?.(false); complete.current = undefined; }
  }, [flow, current, opened]);
  const bound = sameEndpoint(catalog?.endpoint, transport.endpoint);
  const selected = bound ? catalog?.workspaces.find(w => w.id === workspaceId) : undefined;
  const pick = (id: string) => { setWorkspace(id); setMenu(false); };
  const adopt = async (location: string) => {
    if (adopting) return;
    setAdopting(true); setError('');
    try {
      await host.adoptWorkspace(location);
      if (!current()) return;
      const next = await host.listWorkspaces();
      if (!current()) return;
      setCatalog(next); setAdd(false);
      const adopted = next.workspaces.find(w => w.location === location);
      if (adopted) pick(adopted.id);
    } catch (e) { if (current()) setError(String(e)); }
    finally { if (current()) setAdopting(false); }
  };
  const chooseModel = (selection: SessionModelConfig) => {
    setIntent(selection);
    if (modelCommand.current) {
      modelCommand.current = false;
      setDraftConsumed(value => ({ id: 'model', sequence: (value?.sequence ?? 0) + 1 }));
      queueMicrotask(() => seat.current?.querySelector<HTMLTextAreaElement>('textarea')?.focus());
    }
  };
  const composer = (selection: SessionModelConfig | undefined, block: () => string | undefined, permission?: React.ReactNode, model?: React.ReactNode) => <AgentComposer consumed={active ? consumed : draftConsumed} commandAvailable={active ? undefined : id => id === 'model'} onCommand={() => { modelCommand.current = true; seat.current?.querySelector<HTMLButtonElement>('[aria-label="Model and reasoning"]')?.click(); }} submitDisabled={!!block()} busy={!drafting && !flow.matches('failed') && !flow.matches('uncertain_creation')} active={false}
    permission={permission} model={model} onCancel={() => {}} onUpload={async () => { throw new Error('No Session exists before submit.'); }} onSend={async () => false}
    onDraftSend={active ? undefined : async (text, files) => {
      const blocked = block();
      if (blocked) { setError(blocked); return false; }
      if (!selected) { setError('Choose a Host-authorized registered Workspace before submitting.'); return false; }
      return new Promise<boolean>(resolve => { complete.current = resolve; actor.send({ type: 'SUBMIT', port: firstSubmitPort(client, host, current), draft: { workspaceId: selected.id, text, files, model: selection } }); });
    }} {...active} disabled={active ? active.disabled || flow.matches('failed') : !live || owner !== binding || flow.matches('uncertain_creation') || flow.matches('failed')} binding={JSON.stringify([binding, discarded])}/>;
  return <div ref={seat} className={hero.body} data-resident-composer="" role="region" aria-label={active ? "Conversation composer" : "New Conversation"}>
    {!active && <div className={hero.headline}><h1 className={hero.titleGroup}>What would you like to build?</h1></div>}
    {!active && <div className={hero.workspaceRow}>
      <Menu open={menu} autoFocus selectedId={selected?.id} onClose={() => setMenu(false)}
        items={[...(bound ? catalog?.workspaces ?? [] : []).map(w => ({ id: w.id, label: w.displayName, icon: <IconFolderClose16 size={16}/> })), ...(bound && catalog?.picker.kind === 'configured' ? [{ id: '::add-workspace', label: '+ Add Workspace' }] : [])]}
        onSelect={id => { if (id === '::add-workspace') { setMenu(false); setAdd(true); } else pick(id); }}
        anchor={<button type="button" className={hero.workspace} aria-label="Choose Workspace" aria-haspopup="menu" aria-expanded={menu} disabled={!drafting || adopting} onClick={() => setMenu(v => !v)}><IconFolderClose16 size={16}/><span className={hero.workspaceLabel}>{selected?.displayName ?? 'Choose Workspace'}</span><IconChevronDownOutline14 size={12}/></button>}/>
    </div>}
    <ComposerContextStack todo={context} goal={null} queue={null} composer={<WorkspaceControls client={client} host={host} workspaceId={active ? initialWorkspace : workspaceId}>{(source, approval) => { const selection = intent ?? preference ?? (source?.session_models?.kind === 'available' ? source.session_models.default_model : undefined); const block = () => approval.block() ?? sessionModelBlock(source, selection); return <>{composer(selection, block, (active ? initialWorkspace : selected) && <WorkspacePermission source={source} approval={approval} disabled={active ? active.disabled : !drafting}/>, <AgentControls client={client} view={activeView} draft={active ? undefined : { source, intent: selection, choose: chooseModel, disabled: !drafting }}/>)}{!active && selected && block() && <small role="status">{block()}</small>}</>; }}</WorkspaceControls>}/>
    {!active && catalog?.picker.kind === 'unavailable' && !selected && <small role="status">{catalog.picker.reason}</small>}
    {!active && error && <p role="alert">{error}</p>}
    {flow.context.error != null && <p role="alert">{String(flow.context.error)}. {flow.matches('drafting') ? 'No Session was created. Correct the draft or Workspace and submit again.' : flow.context.session ? `Session ${flow.context.session.id} was created; ${flow.context.receipts.length} upload(s) committed. Inspect that Session to recover; no operation was replayed.` : 'Creation outcome uncertain. Inspect native Sessions before starting another conversation. No mutation was replayed.'}</p>}
    {active && flow.matches('failed') && <Button onClick={() => { setDiscarded(value => value + 1); actor.send({ type: 'RESET' }); }}>Discard first-submission draft</Button>}
    <DialogSurface open={add} onClose={() => { if (!adopting) setAdd(false); }} title="Add Workspace" overlayClassName={`${modal.root} ${modal.scrim}`} className={modal.dialog}>
      <div className={modal.content}><div className={modal.header}><h2 className={modal.title}>Add Workspace</h2></div><div className={modal.body}><p>Choose a location authorized by this Product Host.</p>{catalog?.picker.kind === 'configured' && catalog.picker.locations.map(location => <Button key={location.id} disabled={adopting} onClick={() => void adopt(location.id)}>{location.displayName}</Button>)}<Button disabled={adopting} onClick={() => setAdd(false)}>Cancel</Button></div></div>
    </DialogSurface>
  </div>;
}
