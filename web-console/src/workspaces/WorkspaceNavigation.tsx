import { useToastNotice } from '../locale/use-toast-notice';
import type { Translate } from '../locale/translation';
import { useTranslation } from '../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-workspace browser/rows/picker; see PROVENANCE.md. */
import { useEffect, useMemo, useState, useRef, useSyncExternalStore, type ReactNode } from 'react';
import type { AppServerClient } from '../client/app-server';
import type { ClientView } from '../client/contracts';
import { useClientSelector, sameValue, type ShellView } from '../client/selectors';
import { sessionDisplayTitle } from '../bindings/session-title';
import { activeAttempt } from '../bindings/projection';
import { deriveSessionProductState, type SessionTransition } from '../bindings/session-product';
import { actionError } from '../bindings/action-error';
import { Button } from '../presentation/primitives/Button';
import { Input } from '../presentation/primitives/Input';
import { Modal } from '../presentation/primitives/Modal';
import { WorkspaceBrowser } from '../presentation/workspace/WorkspaceBrowser';
import type { DraftConversation, SessionNode, GroupNode } from '../presentation/workspace/types';
import { sameEndpoint } from './endpoint';
import { WorkspaceHostError, type ProductHostWorkspaces, type WorkspaceCatalog } from './host';
import { SessionPins } from './session-pins';
import { RenameSession } from './RenameSession';
import { writeClipboard } from '../presentation/primitives/clipboard';
import { Toast } from '../presentation/primitives/Toast';
import { IconCheckOutline16, IconWarningOutline16 } from '../presentation/primitives/icons';
import type { WorkspaceAssociations, WorkspaceMutation } from './associations';


/** Activity requires a current attachment; cached snapshots cannot claim execution. */
export function sessionObservation(tx: Translate, state: ClientView, id: string, transition?: SessionTransition) {
  const view = state.views[id];
  const product = deriveSessionProductState(tx, state, view, id, transition);
  if (product.status === 'uncertain') return product.label;
  if (state.connection !== 'connected') return '';
  if (!transition && state.connection === 'connected' && (!view || view.attachmentIntent === 'released')) return '';
  return product.label ?? '';
}
export function WorkspaceNavigation({ associations, host, client, state, transition, endpoint, workspace, selected, draft, selectWorkspace, openSession, openViews, closeView, closeAllViews, createSession, forkSession, deleteSession, metadataChanged, wide, expand, workspaceSettings }: {
  associations: WorkspaceAssociations; transition?: { sessionId: string; action: SessionTransition };
  workspaceSettings?: (id: string, label: string) => void;
  host: ProductHostWorkspaces; client: AppServerClient; state: ShellView; endpoint: string;
  wide: boolean; expand: () => void;
  metadataChanged: (removed?: string) => void;
  draft?: DraftConversation;
  workspace?: string; selected?: string; selectWorkspace: (id?: string) => void;
  openViews: readonly string[]; closeView: (id: string) => void; closeAllViews: () => void;
  openSession: (id: string) => void; createSession: (id: string) => void; forkSession: (id: string) => void; deleteSession: (id: string) => void;
}) {
  const tx = useTranslation();
  const associationState = useSyncExternalStore(associations.subscribe, associations.getSnapshot);
  const catalog = associationState.catalog;
  const [query, setQuery] = useState(''), [offset, setOffset] = useState(0);
  const [notice, setError] = useToastNotice();
  const error = notice?.text ?? '';
  const [busy, setBusy] = useState(false);
  const [renaming, setRenaming] = useState<{ id: string; title: string }>();
  const [toast, setToast] = useState<{ sequence: number; copied: boolean }>();
  const pinScope = JSON.stringify([state.endpoint, state.authorityId]);
  const pins = useMemo(() => { let storage: Storage | undefined; try { storage = globalThis.localStorage; } catch {} return new SessionPins(pinScope, storage); }, [pinScope]);
  const pinned = useSyncExternalStore(pins.subscribe, pins.getSnapshot);
  useEffect(() => client.subscribeSessionDeletion(id => pins.remove(id)), [client, pins]);
  const [dialog, setDialog] = useState<{ kind: 'workspace' | 'remove' | 'add'; id: string; name: string; mutation: WorkspaceMutation; catalog: WorkspaceCatalog }>();
  const [name, setName] = useState('');
  const connected = state.connection === 'connected';
  const route = state.endpoint ?? endpoint;
  const bound = sameEndpoint(catalog?.endpoint, route);
  const searchRequest = useRef<object | undefined>(undefined);
  const directory = useRef({ workspace, selected }); directory.current = { workspace, selected };
  const capture = () => {
    const generation = client.getSnapshot().generation, request = searchRequest.current, scope = directory.current;
    return () => generation === client.getSnapshot().generation && request === searchRequest.current && scope.workspace === directory.current.workspace && scope.selected === directory.current.selected;
  };
  const search = (text: string, page = 0) => {
    const request = {}; searchRequest.current = request; const authorityCurrent = capture(); const current = () => searchRequest.current === request && authorityCurrent();
    setQuery(text); setOffset(page); setError('');
    void client.listSessions(page, text, current).catch(cause => { if (current()) setError(actionError(cause)); });
  };
  const edit = (kind: 'workspace' | 'session' | 'remove' | 'add', id: string, title: string) => {
    setError(''); setName(title);
    if (kind === 'session') { setRenaming({ id, title }); return; }
    const mutation = catalog ? associations.captureMutation(catalog) : undefined;
    if (!mutation || !catalog) { setError(String(new WorkspaceHostError('Workspace Host authority replaced', 'authority_replaced'))); return; }
    setDialog({ kind, id, name: title, mutation, catalog });
  };
  const mutate = async (action: () => Promise<unknown>, mutation?: WorkspaceMutation, removed?: string) => {
    const current = capture();
    if (busy) return;
    setBusy(true); setError('');
    try {
      await action();
      const committed = mutation?.commit(removed);
      setDialog(undefined);
      if (committed && current()) metadataChanged(removed);
    }
    catch (cause) { setError(actionError(cause)); }
    finally { setBusy(false); }
  };
  const rank = new Map(pinned.map((id, index) => [id, index]));
  const sessions = [...state.sessions].sort((a, b) => (rank.get(a.id) ?? pinned.length) - (rank.get(b.id) ?? pinned.length));
  const rows = sessions.map(session => {
    const entry = associationState.entries.get(session.id);
    return { session, group: entry?.cwd === session.cwd ? entry.confirmed?.workspaceId : undefined };
  });
  const status = associationState.status;

  const toNode = (session: typeof state.sessions[number]): SessionNode => {
    return { id: session.id, title: sessionDisplayTitle(tx, session), viewOpen: openViews.includes(session.id), pinned: pinned.includes(session.id),
      running: false, runningSubagentCount: 0, forking: transition?.sessionId === session.id && transition.action === 'fork', updatedAt: Date.parse(session.updated_at) || 0 };

  };
  const groupNodes: GroupNode[] = (bound ? catalog?.workspaces ?? [] : []).map(row => ({ key: row.id, workspaceId: row.id,
    cwd: row.displayPath, createdAt: undefined, label: row.displayName, expanded: true,
    containsCurrent: row.id === workspace,
    sessionCount: rows.filter(item => item.group === row.id).length, sessions: rows.filter(item => item.group === row.id).map(item => toNode(item.session)) }));
  return <>
    <WorkspaceBrowser draft={bound && !selected && catalog?.workspaces.some(row => row.id === draft?.workspaceId) ? draft : undefined} renderSession={(node, render) => <LiveSessionNode key={node.id} client={client} node={node} transition={transition?.sessionId === node.id ? transition.action : undefined}>{render}</LiveSessionNode>} wide={wide} expand={expand} groups={groupNodes} sessions={sessions.map(toNode)} selected={selected} closeView={closeView} closeAllViews={openViews.length ? closeAllViews : undefined}
      unclassified={state.sessions.filter(session => { const entry = associationState.entries.get(session.id); return !entry || entry.cwd !== session.cwd || (!entry.confirmed && entry.status !== 'revoked'); }).map(session => session.id)} query={query} search={text => connected && search(text)} open={id => connected && openSession(id)} rename={id => edit('session', id, state.sessions.find(session => session.id === id)?.name ?? state.sessions.find(session => session.id === id)?.preview ?? '')} pin={id => pins.toggle(id)} copyId={id => { const generation = client.getSnapshot().generation; void writeClipboard(id).then(copied => { if (generation === client.getSnapshot().generation) setToast(previous => ({ sequence: (previous?.sequence ?? 0) + 1, copied })); }); }} fork={forkSession} remove={deleteSession}
      workspaceSettings={workspaceSettings} selectWorkspace={selectWorkspace} create={id => connected && createSession(id)} renameWorkspace={(id, title) => edit('workspace', id, title)} removeWorkspace={(id, title) => edit('remove', id, title)}
      addWorkspace={catalog?.picker.kind === 'configured' && bound ? () => edit('add', '', '') : undefined}
      refresh={() => { associations.refresh(); search(query, offset); }} previous={connected && offset > 0 ? () => search(query, Math.max(0, offset - 32)) : undefined}
      next={connected && state.nextOffset != null ? () => search(query, state.nextOffset!) : undefined}
      notices={<>{status && status !== 'ready' && status !== 'disconnected' && <p role="status">{tx(status === 'unavailable' ? 'workspace:association.unavailable' : 'workspace:association.refreshing')}</p>}{catalog && !bound && <p role="status">{tx('workspace:workspace-navigation.this-workspace-host-belongs-to')}{' '}{catalog.endpoint}.</p>}</>} />
    {!dialog && error && <Toast key={notice?.sequence} text={error} holdMs={6000} icon={<IconWarningOutline16/>} onDone={() => setError('')}/>}
    {renaming && <RenameSession key={renaming.id} client={client} id={renaming.id} title={renaming.title} close={() => setRenaming(undefined)}/>}
    {toast && <Toast key={toast.sequence} anchor={null} tone={toast.copied ? 'success' : undefined} text={tx(toast.copied ? 'workspace:toast.sessionIdCopied' : 'workspace:toast.sessionIdCopyFailed')} icon={toast.copied ? <IconCheckOutline16/> : <IconWarningOutline16/>} onDone={() => setToast(undefined)}/>}
    {dialog && <Modal closeLabel={tx('workspace:workspace-navigation.close-dialog')} open title={dialog.kind === 'add' ? tx('workspace:workspace-navigation.add-workspace') : dialog.kind === 'remove' ? tx('workspace:workspace-navigation.unregister-value', { p0: dialog.name }) : tx('workspace:rename.workspace')} onClose={() => { if (!busy) setDialog(undefined); }}>
      {error && <p role="alert">{error}</p>}
      {dialog.kind === 'add' ? <><p>{tx('workspace:workspace-navigation.choose-a-location-authorized-by-this-product-host')}</p>{dialog.catalog.picker.kind === 'configured' && dialog.catalog.picker.locations.map(location => <Button key={location.id} disabled={busy} onClick={() => void mutate(() => host.adoptWorkspace(dialog.mutation.scope, location.id), dialog.mutation)}>{location.displayName}</Button>)}</>
          : dialog.kind === 'remove' ? <><p>{tx('workspace:workspace-navigation.only-the-navigation-registration-is-removed-sessions-cwd-history')}</p><Button disabled={busy} onClick={() => void mutate(() => host.removeWorkspace(dialog.mutation.scope, dialog.id), dialog.mutation, dialog.id)}>{tx('workspace:workspace-navigation.unregister')}</Button></>
            : <form onSubmit={event => { event.preventDefault(); void mutate(async () => {
              await host.renameWorkspace(dialog.mutation.scope, dialog.id, name);
            }, dialog.mutation); }}><Input autoFocus disabled={busy} onKeyDown={event => { if (event.key === 'Enter' && (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229)) event.preventDefault(); }} aria-label={tx('workspace:workspace-navigation.name')} value={name} onChange={event => setName(event.target.value)} /><Button type="submit" disabled={busy || !name.trim()}>{tx('workspace:workspace-navigation.save-name')}</Button></form>}
    </Modal>}
  </>;
}

/** Only the individual activity row observes execution; the browser stays catalog-owned. */
function LiveSessionNode({ client, node, children, transition }: { client: AppServerClient; node: SessionNode; transition?: SessionTransition; children: (node: SessionNode) => ReactNode }) {
  const tx = useTranslation();
  const activity = useClientSelector(client, state => {
    const view = state.views[node.id];
    const current = state.connection === 'connected' && view?.attachment === 'attached' && view.attachmentIntent === 'wanted';
    const pending = current ? view.snapshot?.pending_interactions?.[0] : undefined;
    return { running: !!current && activeAttempt(view.snapshot),
      pendingInteraction: pending ? pending.kind === 'approval' ? 'approval' as const : pending.kind === 'review' ? 'plan-review' as const : 'question' as const : undefined,
      observation: sessionObservation(tx, state, node.id, transition) };
  }, sameValue);
  return children({ ...node, ...activity });
}
