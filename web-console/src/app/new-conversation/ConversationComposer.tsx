import { message, displayText, type Message } from '../../locale/translation';
import { useTranslation, useNotice } from '../../locale/react';
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
  const tx = useTranslation();
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
  const [error, setError] = useNotice(), [menu, setMenu] = useState(false), [add, setAdd] = useState(false), [adopting, setAdopting] = useState(false);
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
      opened(flow.context.session.id, tx('common:copy.session-created-first-submission-stopped-value-value-upload-s-committed-no-operation-was-r', { p0: String(flow.context.error), p1: flow.context.receipts.length }));
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
  const composer = (selection: SessionModelConfig | undefined, block: () => Message | undefined, permission?: React.ReactNode, model?: React.ReactNode) => <AgentComposer consumed={active ? consumed : draftConsumed} commandAvailable={active ? undefined : id => id === 'model'} onCommand={() => { modelCommand.current = true; seat.current?.querySelector<HTMLButtonElement>('[data-model-select]')?.click(); }} submitDisabled={!!block()} busy={!drafting && !flow.matches('failed') && !flow.matches('uncertain_creation')} active={false}
    permission={permission} model={model} onCancel={() => {}} onUpload={async () => { throw new Error(tx('common:copy.no-session-exists-before-submit')); }} onSend={async () => false}
    onDraftSend={active ? undefined : async (text, files) => {
      const blocked = block();
      if (blocked) { setError(blocked); return false; }
      if (!selected) { setError(message('common:copy.choose-a-host-authorized-registered-workspace-before-submitting')); return false; }
      return new Promise<boolean>(resolve => { complete.current = resolve; actor.send({ type: 'SUBMIT', port: firstSubmitPort(client, host, current), draft: { workspaceId: selected.id, text, files, model: selection } }); });
    }} {...active} disabled={active ? active.disabled || flow.matches('failed') : !live || owner !== binding || flow.matches('uncertain_creation') || flow.matches('failed')} binding={JSON.stringify([binding, discarded])}/>;
  return <div ref={seat} className={hero.body} data-resident-composer="" role="region" aria-label={active ? tx('common:conversation-composer.conversation-composer') : tx('common:conversation-composer.new-conversation')}>
    {!active && <div className={hero.headline}><h1 className={hero.titleGroup}>{tx('common:conversation-composer.what-would-you-like-to-build')}</h1></div>}
    {!active && <div className={hero.workspaceRow}>
      <Menu open={menu} autoFocus selectedId={selected?.id} onClose={() => setMenu(false)}
        items={[...(bound ? catalog?.workspaces ?? [] : []).map(w => ({ id: w.id, label: w.displayName, icon: <IconFolderClose16 size={16}/> })), ...(bound && catalog?.picker.kind === 'configured' ? [{ id: '::add-workspace', label: tx('common:conversation-composer.add-workspace') }] : [])]}
        onSelect={id => { if (id === '::add-workspace') { setMenu(false); setAdd(true); } else pick(id); }}
        anchor={<button type="button" className={hero.workspace} aria-label={tx('common:conversation-composer.choose-workspace')} aria-haspopup="menu" aria-expanded={menu} disabled={!drafting || adopting} onClick={() => setMenu(v => !v)}><IconFolderClose16 size={16}/><span className={hero.workspaceLabel}>{selected?.displayName ?? tx('common:conversation-composer.choose-workspace')}</span><IconChevronDownOutline14 size={12}/></button>}/>
    </div>}
    <ComposerContextStack todo={context} goal={null} queue={null} composer={<WorkspaceControls client={client} host={host} workspaceId={active ? initialWorkspace : workspaceId}>{(source, approval) => { const selection = intent ?? preference ?? (source?.session_models?.kind === 'available' ? source.session_models.default_model : undefined); const block = () => approval.block() ?? sessionModelBlock(source, selection); return <>{composer(selection, block, (active ? initialWorkspace : selected) && <WorkspacePermission source={source} approval={approval} disabled={active ? active.disabled : !drafting}/>, <AgentControls client={client} view={activeView} draft={active ? undefined : { source, intent: selection, choose: chooseModel, disabled: !drafting }}/>)}{!active && selected && block() && <small role="status">{displayText(tx, block()!)}</small>}</>; }}</WorkspaceControls>}/>
    {!active && catalog?.picker.kind === 'unavailable' && !selected && <small role="status">{catalog.picker.reason}</small>}
    {!active && error && <p role="alert">{error}</p>}
    {flow.context.error != null && <p role="alert">{String(flow.context.error)}. {flow.matches('drafting') ? tx('common:conversation-composer.no-session-was-created-correct-the-draft-or-workspace-and-submit') : flow.context.session ? tx('common:conversation-composer.session-value-was-created-value-upload-s-committed-inspect-that', { p0: flow.context.session.id, p1: flow.context.receipts.length }) : tx('common:conversation-composer.creation-outcome-uncertain-inspect-native-sessions-before-starti')}</p>}
    {active && flow.matches('failed') && <Button onClick={() => { setDiscarded(value => value + 1); actor.send({ type: 'RESET' }); }}>{tx('common:conversation-composer.discard-first-submission-draft')}</Button>}
    <DialogSurface open={add} onClose={() => { if (!adopting) setAdd(false); }} title={tx('common:conversation-composer.add-workspace-2')} overlayClassName={`${modal.root} ${modal.scrim}`} className={modal.dialog}>
      <div className={modal.content}><div className={modal.header}><h2 className={modal.title}>{tx('common:conversation-composer.add-workspace-2')}</h2></div><div className={modal.body}><p>{tx('common:conversation-composer.choose-a-location-authorized-by-this-product-host')}</p>{catalog?.picker.kind === 'configured' && catalog.picker.locations.map(location => <Button key={location.id} disabled={adopting} onClick={() => void adopt(location.id)}>{location.displayName}</Button>)}<Button disabled={adopting} onClick={() => setAdd(false)}>{tx('common:conversation-composer.cancel')}</Button></div></div>
    </DialogSurface>
  </div>;
}
