import type { Translate } from '../locale/translation';
import { useTranslation } from '../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-workspace browser/rows/picker; see PROVENANCE.md. */
import { useEffect, useState, type ReactNode } from 'react';
import type { AppServerClient, ClientView } from '../client/app-server';
import { useClientSelector, sameValue, type ShellView } from '../client/selectors';
import { sessionDisplayTitle } from '../bindings/session-title';
import { activeAttempt } from '../bindings/projection';
import { deriveSessionProductState } from '../bindings/session-product';
import { NavigationEpoch } from '../app/commands/native';
import { Button } from '../presentation/primitives/Button';
import { Input } from '../presentation/primitives/Input';
import { Modal } from '../presentation/primitives/Modal';
import { WorkspaceBrowser } from '../presentation/workspace/WorkspaceBrowser';
import type { SessionNode, GroupNode } from '../presentation/workspace/types';
import { sameEndpoint } from './endpoint';
import type { ProductHostWorkspaces, WorkspaceCatalog, SessionLocation } from './host';


/** Activity requires a current attachment; cached snapshots cannot claim execution. */
export function sessionObservation(tx: Translate, state: ClientView, id: string) {
  const view = state.views[id];
  const product = deriveSessionProductState(tx, state, view, id);
  if (product.status === 'uncertain') return product.label;
  if (state.connection === 'connected' && (!view || view.attachmentIntent === 'released')) return '';
  return product.label ?? '';
}
export function WorkspaceNavigation({ host, client, state, endpoint, navigation, workspace, selected, selectWorkspace, openSession, openViews, closeView, closeAllViews, createSession, forkSession, deleteSession, metadataChanged, wide, expand, workspaceSettings }: {
  workspaceSettings?: (id: string, label: string) => void;
  host: ProductHostWorkspaces; client: AppServerClient; state: ShellView; endpoint: string; navigation: NavigationEpoch;
  wide: boolean; expand: () => void;
  metadataChanged: (removed?: string) => void;
  workspace?: string; selected?: string; selectWorkspace: (id?: string) => void;
  openViews: readonly string[]; closeView: (id: string) => void; closeAllViews: () => void;
  openSession: (id: string) => void; createSession: (id: string) => void; forkSession: (id: string) => void; deleteSession: (id: string) => void;
}) {
  const tx = useTranslation();
  const [catalog, setCatalog] = useState<WorkspaceCatalog>();
  const [groups, setGroups] = useState<{ sessions: ClientView['sessions']; locations: SessionLocation[] }>();
  const [query, setQuery] = useState(''), [offset, setOffset] = useState(0);
  const [hostError, setHostError] = useState('');
  const [error, setError] = useState(''), [busy, setBusy] = useState(false), [reload, setReload] = useState(0);
  const [dialog, setDialog] = useState<{ kind: 'workspace' | 'session' | 'remove' | 'add'; id: string; name: string }>();
  const [name, setName] = useState('');
  const connected = state.connection === 'connected';
  const route = state.endpoint ?? endpoint;
  const bound = sameEndpoint(catalog?.endpoint, route);
  useEffect(() => {
    let current = true;
    void host.listWorkspaces().then(value => { if (current) { setCatalog(value); setHostError(''); } }, cause => { if (current) { setCatalog(undefined); setHostError(String(cause)); } });
    return () => { current = false; };
  }, [host, reload]);
  useEffect(() => {
    let alive = true;
    setGroups(undefined);
    if (bound && connected) void host.classifyLocations(state.sessions.map(row => row.cwd), route).then(value => { if (alive) setGroups({ sessions: state.sessions, locations: value }); }, cause => { if (alive) setError(String(cause)); });
    return () => { alive = false; };
  }, [host, catalog, state.sessions, route, connected, bound]);
  const search = (text: string, page = 0) => {
    navigation.invalidate(); const current = navigation.capture();
    setQuery(text); setOffset(page); setError('');
    void client.listSessions(page, text, current).catch(cause => { if (current()) setError(String(cause)); });
  };
  const edit = (kind: 'workspace' | 'session' | 'remove', id: string, title: string) => { setError(''); setName(title); setDialog({ kind, id, name: title }); };
  const mutate = async (action: () => Promise<unknown>, metadata: boolean | string = true) => {
    const current = navigation.capture();
    if (busy) return;
    setBusy(true); setError('');
    try { await action(); setReload(value => value + 1); setDialog(undefined); if (metadata && current()) metadataChanged(typeof metadata === 'string' ? metadata : undefined); }
    catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  const locations = groups?.sessions === state.sessions ? groups.locations : [];
  const rows = state.sessions.map((session, index) => ({ session, location: locations[index], group: locations[index]?.authorized ? locations[index].workspaceId ?? null : null }));

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
      query={query} search={text => connected && search(text)} open={id => connected && openSession(id)} rename={id => edit('session', id, state.sessions.find(session => session.id === id)?.name ?? '')} fork={forkSession} remove={deleteSession}
      workspaceSettings={workspaceSettings} selectWorkspace={selectWorkspace} create={id => connected && createSession(id)} renameWorkspace={(id, title) => edit('workspace', id, title)} removeWorkspace={(id, title) => edit('remove', id, title)}
      addWorkspace={catalog?.picker.kind === 'configured' && bound ? () => setDialog({ kind: 'add', id: '', name: '' }) : undefined}
      refresh={() => { setReload(value => value + 1); search(query, offset); }} previous={connected && offset > 0 ? () => search(query, Math.max(0, offset - 32)) : undefined}
      next={connected && state.nextOffset != null ? () => search(query, state.nextOffset!) : undefined}
      notices={<>{hostError && <p role="status">{hostError}</p>}{!dialog && error && <p role="alert">{error}</p>}{catalog && !bound && <p role="status">{tx('workspace:workspace-navigation.this-workspace-host-belongs-to')}{' '}{catalog.endpoint}.</p>}</>} />
    {dialog && <Modal closeLabel={tx('workspace:workspace-navigation.close-dialog')} open title={dialog.kind === 'add' ? tx('workspace:workspace-navigation.add-workspace') : dialog.kind === 'remove' ? tx('workspace:workspace-navigation.unregister-value', { p0: dialog.name }) : tx(dialog.kind === 'workspace' ? 'workspace:rename.workspace' : 'workspace:rename.session')} onClose={() => { if (!busy) setDialog(undefined); }}>
      {error && <p role="alert">{error}</p>}
      {dialog.kind === 'add' ? <><p>{tx('workspace:workspace-navigation.choose-a-location-authorized-by-this-product-host')}</p>{catalog?.picker.kind === 'configured' && catalog.picker.locations.map(location => <Button key={location.id} disabled={busy} onClick={() => void mutate(() => host.adoptWorkspace(location.id))}>{location.displayName}</Button>)}</>
          : dialog.kind === 'remove' ? <><p>{tx('workspace:workspace-navigation.only-the-navigation-registration-is-removed-sessions-cwd-history')}</p><Button disabled={busy} onClick={() => void mutate(() => host.removeWorkspace(dialog.id), dialog.id)}>{tx('workspace:workspace-navigation.unregister')}</Button></>
            : <form onSubmit={event => { event.preventDefault(); const current = navigation.capture(); void mutate(async () => {
              if (dialog.kind === 'workspace') await host.renameWorkspace(dialog.id, name);
              else { await client.renameSession(dialog.id, name); if (current()) await client.listSessions(offset, query, current); }
            }, dialog.kind === 'workspace'); }}><Input autoFocus disabled={busy} onKeyDown={event => { if (event.key === 'Enter' && (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229)) event.preventDefault(); }} aria-label={tx('workspace:workspace-navigation.name')} value={name} onChange={event => setName(event.target.value)} /><Button type="submit" disabled={busy || !name.trim()}>{tx('workspace:workspace-navigation.save-name')}</Button></form>}
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
