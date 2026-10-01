import type { Translate } from '../locale/translation';
import { useTranslation } from '../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-workspace browser/rows/picker; see PROVENANCE.md. */
import { useState, useSyncExternalStore, type ReactNode } from 'react';
import type { AppServerClient, ClientView } from '../client/app-server';
import { useClientSelector, sameValue, type ShellView } from '../client/selectors';
import { sessionDisplayTitle } from '../bindings/session-title';
import { activeAttempt } from '../bindings/projection';
import { deriveSessionProductState } from '../bindings/session-product';
import { NavigationEpoch } from '../client/navigation';
import { Button } from '../presentation/primitives/Button';
import { Input } from '../presentation/primitives/Input';
import { Modal } from '../presentation/primitives/Modal';
import { WorkspaceBrowser } from '../presentation/workspace/WorkspaceBrowser';
import type { SessionNode, GroupNode } from '../presentation/workspace/types';
import { sameEndpoint } from './endpoint';
import { WorkspaceHostError, type ProductHostWorkspaces, type WorkspaceCatalog } from './host';
import type { WorkspaceAssociations, WorkspaceMutation } from './associations';


/** Activity requires a current attachment; cached snapshots cannot claim execution. */
export function sessionObservation(tx: Translate, state: ClientView, id: string) {
  const view = state.views[id];
  const product = deriveSessionProductState(tx, state, view, id);
  if (product.status === 'uncertain') return product.label;
  if (state.connection === 'connected' && (!view || view.attachmentIntent === 'released')) return '';
  return product.label ?? '';
}
export function WorkspaceNavigation({ associations, host, client, state, endpoint, navigation, workspace, selected, selectWorkspace, openSession, openViews, closeView, closeAllViews, createSession, forkSession, deleteSession, metadataChanged, wide, expand, workspaceSettings }: {
  associations: WorkspaceAssociations;
  workspaceSettings?: (id: string, label: string) => void;
  host: ProductHostWorkspaces; client: AppServerClient; state: ShellView; endpoint: string; navigation: NavigationEpoch;
  wide: boolean; expand: () => void;
  metadataChanged: (removed?: string) => void;
  workspace?: string; selected?: string; selectWorkspace: (id?: string) => void;
  openViews: readonly string[]; closeView: (id: string) => void; closeAllViews: () => void;
  openSession: (id: string) => void; createSession: (id: string) => void; forkSession: (id: string) => void; deleteSession: (id: string) => void;
}) {
  const tx = useTranslation();
  const associationState = useSyncExternalStore(associations.subscribe, associations.getSnapshot);
  const catalog = associationState.catalog;
  const [query, setQuery] = useState(''), [offset, setOffset] = useState(0);
  const [error, setError] = useState(''), [busy, setBusy] = useState(false);
  const [dialog, setDialog] = useState<{ kind: 'session'; id: string; name: string }
    | { kind: 'workspace' | 'remove' | 'add'; id: string; name: string; mutation: WorkspaceMutation; catalog: WorkspaceCatalog }>();
  const [name, setName] = useState('');
  const connected = state.connection === 'connected';
  const route = state.endpoint ?? endpoint;
  const bound = sameEndpoint(catalog?.endpoint, route);
  const search = (text: string, page = 0) => {
    navigation.invalidate(); const current = navigation.capture();
    setQuery(text); setOffset(page); setError('');
    void client.listSessions(page, text, current).catch(cause => { if (current()) setError(String(cause)); });
  };
  const edit = (kind: 'workspace' | 'session' | 'remove' | 'add', id: string, title: string) => {
    setError(''); setName(title);
    if (kind === 'session') { setDialog({ kind, id, name: title }); return; }
    const mutation = catalog ? associations.captureMutation(catalog) : undefined;
    if (!mutation || !catalog) { setError(String(new WorkspaceHostError('Workspace Host authority replaced', 'authority_replaced'))); return; }
    setDialog({ kind, id, name: title, mutation, catalog });
  };
  const mutate = async (action: () => Promise<unknown>, mutation?: WorkspaceMutation, removed?: string) => {
    const current = navigation.capture();
    if (busy) return;
    setBusy(true); setError('');
    try {
      await action();
      const committed = mutation?.commit(removed);
      setDialog(undefined);
      if (committed && current()) metadataChanged(removed);
    }
    catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  const rows = state.sessions.map(session => {
    const entry = associationState.entries.get(session.id);
    return { session, group: entry?.cwd === session.cwd ? entry.confirmed?.workspaceId : undefined };
  });
  const status = associationState.status;

  const toNode = (session: typeof state.sessions[number]): SessionNode => {
    return { id: session.id, title: sessionDisplayTitle(tx, session), viewOpen: openViews.includes(session.id),
      running: false, runningSubagentCount: 0, updatedAt: Date.parse(session.updated_at) || 0 };

  };
  const groupNodes: GroupNode[] = (bound ? catalog?.workspaces ?? [] : []).map(row => ({ key: row.id, workspaceId: row.id,
    cwd: row.displayPath, createdAt: undefined, label: row.displayName, expanded: true,
    containsCurrent: row.id === workspace,
    sessionCount: rows.filter(item => item.group === row.id).length, sessions: rows.filter(item => item.group === row.id).map(item => toNode(item.session)) }));
  return <>
    <WorkspaceBrowser renderSession={(node, render) => <LiveSessionNode key={node.id} client={client} node={node}>{render}</LiveSessionNode>} wide={wide} expand={expand} groups={groupNodes} sessions={state.sessions.map(toNode)} selected={selected} closeView={closeView} closeAllViews={openViews.length ? closeAllViews : undefined}
      unclassified={state.sessions.filter(session => { const entry = associationState.entries.get(session.id); return !entry || entry.cwd !== session.cwd || (!entry.confirmed && entry.status !== 'revoked'); }).map(session => session.id)} query={query} search={text => connected && search(text)} open={id => connected && openSession(id)} rename={id => edit('session', id, state.sessions.find(session => session.id === id)?.name ?? '')} fork={forkSession} remove={deleteSession}
      workspaceSettings={workspaceSettings} selectWorkspace={selectWorkspace} create={id => connected && createSession(id)} renameWorkspace={(id, title) => edit('workspace', id, title)} removeWorkspace={(id, title) => edit('remove', id, title)}
      addWorkspace={catalog?.picker.kind === 'configured' && bound ? () => edit('add', '', '') : undefined}
      refresh={() => { associations.refresh(); search(query, offset); }} previous={connected && offset > 0 ? () => search(query, Math.max(0, offset - 32)) : undefined}
      next={connected && state.nextOffset != null ? () => search(query, state.nextOffset!) : undefined}
      notices={<>{status && status !== 'ready' && <p role="status">{tx(status === 'disconnected' ? 'workspace:association.disconnected' : status === 'unavailable' ? 'workspace:association.unavailable' : 'workspace:association.refreshing')}</p>}{!dialog && error && <p role="alert">{error}</p>}{catalog && !bound && <p role="status">{tx('workspace:workspace-navigation.this-workspace-host-belongs-to')}{' '}{catalog.endpoint}.</p>}</>} />
    {dialog && <Modal closeLabel={tx('workspace:workspace-navigation.close-dialog')} open title={dialog.kind === 'add' ? tx('workspace:workspace-navigation.add-workspace') : dialog.kind === 'remove' ? tx('workspace:workspace-navigation.unregister-value', { p0: dialog.name }) : tx(dialog.kind === 'workspace' ? 'workspace:rename.workspace' : 'workspace:rename.session')} onClose={() => { if (!busy) setDialog(undefined); }}>
      {error && <p role="alert">{error}</p>}
      {dialog.kind === 'add' ? <><p>{tx('workspace:workspace-navigation.choose-a-location-authorized-by-this-product-host')}</p>{dialog.catalog.picker.kind === 'configured' && dialog.catalog.picker.locations.map(location => <Button key={location.id} disabled={busy} onClick={() => void mutate(() => host.adoptWorkspace(dialog.mutation.scope, location.id), dialog.mutation)}>{location.displayName}</Button>)}</>
          : dialog.kind === 'remove' ? <><p>{tx('workspace:workspace-navigation.only-the-navigation-registration-is-removed-sessions-cwd-history')}</p><Button disabled={busy} onClick={() => void mutate(() => host.removeWorkspace(dialog.mutation.scope, dialog.id), dialog.mutation, dialog.id)}>{tx('workspace:workspace-navigation.unregister')}</Button></>
            : <form onSubmit={event => { event.preventDefault(); const current = navigation.capture(); void mutate(async () => {
              if (dialog.kind === 'workspace') await host.renameWorkspace(dialog.mutation.scope, dialog.id, name);
              else { await client.renameSession(dialog.id, name); if (current()) await client.listSessions(offset, query, current); }
            }, dialog.kind === 'workspace' ? dialog.mutation : undefined); }}><Input autoFocus disabled={busy} onKeyDown={event => { if (event.key === 'Enter' && (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229)) event.preventDefault(); }} aria-label={tx('workspace:workspace-navigation.name')} value={name} onChange={event => setName(event.target.value)} /><Button type="submit" disabled={busy || !name.trim()}>{tx('workspace:workspace-navigation.save-name')}</Button></form>}
    </Modal>}
  </>;
}

/** Only the individual activity row observes execution; the browser stays catalog-owned. */
function LiveSessionNode({ client, node, children }: { client: AppServerClient; node: SessionNode; children: (node: SessionNode) => ReactNode }) {
  const tx = useTranslation();
  const activity = useClientSelector(client, state => {
    const view = state.views[node.id];
    const current = state.connection === 'connected' && view?.attachment === 'attached' && view.attachmentIntent === 'wanted';
    const pending = current ? view.snapshot?.pending_interactions?.[0] : undefined;
    return { running: !!current && activeAttempt(view.snapshot),
      pendingInteraction: pending ? pending.request.kind.type === 'approval' ? 'approval' as const : pending.request.kind.type === 'review' ? 'plan-review' as const : 'question' as const : undefined,
      observation: sessionObservation(tx, state, node.id) };
  }, sameValue);
  return children({ ...node, ...activity });
}
