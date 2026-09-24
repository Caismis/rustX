/* Copyright (c) 2026 DeepSeek. MIT. Adapted EmptyHero and WorkspacePicker; see PROVENANCE.md. */
import { useEffect, useMemo, useState } from 'react';
import { useActorRef, useSelector } from '@xstate/react';
import type { SessionModelConfig } from '../../../../protocol/app-server/v21';
import type { AppServerClient } from '../../client/app-server';
import type { ProductHostWorkspaces, WorkspaceCatalog } from '../../workspaces/host';
import { sameEndpoint } from '../../workspaces/endpoint';
import { Menu } from '../../presentation/primitives/Menu';
import { Button } from '../../presentation/primitives/Button';
import { DialogSurface } from '../../presentation/primitives/DialogSurface';
import { IconChevronDownOutline14, IconFolderClose16 } from '../../presentation/primitives/icons';
import hero from '../../presentation/agent/HeroShell.module.css';
import modal from '../../presentation/primitives/Modal.module.css';
import { AgentComposer } from '../agent/AgentComposer';
import { firstSubmitMachine } from './first-submit';
import { firstSubmitPort } from './port';
import { NewConversationModelControl, WorkspaceControls, WorkspacePermission } from './WorkspaceControls';

export function NewConversation({ client, host, initialWorkspace, current, opened }: { client: AppServerClient; host: ProductHostWorkspaces; initialWorkspace?: string; current: () => boolean; opened: (id: string, error?: string) => void }) {
  const [workspaceId, setWorkspace] = useState(initialWorkspace);
  const [intent, setIntent] = useState<SessionModelConfig>();
  const [catalog, setCatalog] = useState<WorkspaceCatalog>();
  const [error, setError] = useState(''), [menu, setMenu] = useState(false), [add, setAdd] = useState(false), [adopting, setAdopting] = useState(false);
  const port = useMemo(() => firstSubmitPort(client, host, current), [client, host, current]);
  const actor = useActorRef(firstSubmitMachine, { input: { port } });
  const flow = useSelector(actor, s => s);
  const drafting = flow.matches('drafting');
  useEffect(() => {
    let alive = true;
    void host.listWorkspaces().then(value => { if (alive && current()) setCatalog(value); }, e => { if (alive && current()) setError(String(e)); });
    return () => { alive = false; };
  }, [host, current]);
  useEffect(() => {
    if (!port.current()) return;
    if (flow.matches('session')) opened(flow.context.session!.id);
    // A later failure never rewinds creation: the committed Session is opened.
    else if (flow.matches('failed') && flow.context.session) opened(flow.context.session.id, `Session created. First submission stopped: ${String(flow.context.error)}. ${flow.context.receipts.length} upload(s) committed. No operation was replayed.`);
  }, [flow, opened, port]);
  const bound = sameEndpoint(catalog?.endpoint, client.getSnapshot().endpoint);
  const selected = bound ? catalog?.workspaces.find(w => w.id === workspaceId) : undefined;
  const pick = (id: string) => { setWorkspace(id); setIntent(undefined); setMenu(false); };
  const adopt = async (location: string) => {
    if (adopting) return;
    setAdopting(true); setError('');
    try {
      await host.adoptWorkspace(location);
      if (!port.current()) return;
      const next = await host.listWorkspaces();
      if (!port.current()) return;
      setCatalog(next); setAdd(false);
      const adopted = next.workspaces.find(w => w.location === location);
      if (adopted) pick(adopted.id);
    } catch (e) { if (port.current()) setError(String(e)); }
    finally { if (port.current()) setAdopting(false); }
  };
  const composer = (permission?: React.ReactNode, model?: React.ReactNode) => <AgentComposer disabled={!drafting || !port.current()} busy={!drafting && !flow.matches('failed')} active={false}
    permission={permission} model={model} onCancel={() => {}} onUpload={async () => { throw new Error('No Session exists before submit.'); }} onSend={async () => false}
    onDraftSend={async (text, files) => {
      if (!selected) { setError('Choose a Host-authorized registered Workspace before submitting.'); return false; }
      actor.send({ type: 'SUBMIT', draft: { workspaceId: selected.id, text, files, model: intent } }); return false;
    }}/>;
  return <section className={hero.root} aria-label="New Conversation"><div className={hero.stack}>
    <div className={hero.headline}><h1 className={hero.titleGroup}>What would you like to build?</h1></div>
    <div className={hero.body}><div className={hero.workspaceRow}>
      <Menu open={menu} autoFocus selectedId={selected?.id} onClose={() => setMenu(false)}
        items={[...(bound ? catalog?.workspaces ?? [] : []).map(w => ({ id: w.id, label: w.displayName, icon: <IconFolderClose16 size={16}/> })), ...(bound && catalog?.picker.kind === 'configured' ? [{ id: '::add-workspace', label: '+ Add Workspace' }] : [])]}
        onSelect={id => { if (id === '::add-workspace') { setMenu(false); setAdd(true); } else pick(id); }}
        anchor={<button type="button" className={hero.workspace} aria-label="Choose Workspace" aria-haspopup="menu" aria-expanded={menu} disabled={!drafting || adopting} onClick={() => setMenu(v => !v)}><IconFolderClose16 size={16}/><span className={hero.workspaceLabel}>{selected?.displayName ?? 'Choose Workspace'}</span><IconChevronDownOutline14 size={12}/></button>}/>
    </div>
    {<WorkspaceControls client={client} host={host} workspaceId={workspaceId}>{source => composer(selected && <WorkspacePermission key={workspaceId} source={source} disabled={!drafting}/>, selected && <NewConversationModelControl key={workspaceId} source={source} intent={intent} choose={setIntent} disabled={!drafting}/>)}</WorkspaceControls>}
    </div>
    {catalog?.picker.kind === 'unavailable' && !selected && <small role="status">{catalog.picker.reason}</small>}
    {error && <p role="alert">{error}</p>}
    {flow.matches('failed') && <p role="alert">{String(flow.context.error)}. Inspect native Sessions before starting another conversation. No mutation was replayed.</p>}
    {!drafting && !flow.matches('failed') && <small role="status">{String(flow.value).replaceAll('_', ' ')}…</small>}
    <DialogSurface open={add} onClose={() => { if (!adopting) setAdd(false); }} title="Add Workspace" overlayClassName={`${modal.root} ${modal.scrim}`} className={modal.dialog}>
      <div className={modal.content}><div className={modal.header}><h2 className={modal.title}>Add Workspace</h2></div><div className={modal.body}><p>Choose a location authorized by this Product Host.</p>{catalog?.picker.kind === 'configured' && catalog.picker.locations.map(location => <Button key={location.id} disabled={adopting} onClick={() => void adopt(location.id)}>{location.displayName}</Button>)}<Button disabled={adopting} onClick={() => setAdd(false)}>Cancel</Button></div></div>
    </DialogSurface>
  </div></section>;
}
