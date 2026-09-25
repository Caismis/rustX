/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-workspace browser/rows/picker; see PROVENANCE.md. */
import { useEffect, useState } from 'react';
import type { AppServerClient, ClientView } from '../client/app-server';
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
export function sessionObservation(state: ClientView, id: string) {
  const view = state.views[id];
  const product = deriveSessionProductState(state, view, id);
  if (product.status === 'uncertain') return product.label;
  if (state.connection === 'connected' && (!view || view.attachmentIntent === 'released')) return '';
  return product.label ?? '';
}
export function WorkspaceNavigation({ host, client, state, endpoint, navigation, workspace, selected, selectWorkspace, openSession, openViews, closeView, closeAllViews, createSession, forkSession, deleteSession, metadataChanged, wide, expand, workspaceSettings }: {
  workspaceSettings?: (id: string, label: string) => void;
  host: ProductHostWorkspaces; client: AppServerClient; state: ClientView; endpoint: string; navigation: NavigationEpoch;
  wide: boolean; expand: () => void;
  metadataChanged: (removed?: string) => void;
  workspace?: string; selected?: string; selectWorkspace: (id?: string) => void;
  openViews: readonly string[]; closeView: (id: string) => void; closeAllViews: () => void;
  openSession: (id: string) => void; createSession: (id: string) => void; forkSession: (id: string) => void; deleteSession: (id: string) => void;
}) {
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
    const view = state.views[session.id];
    const current = connected && view?.attachment === 'attached' && view.attachmentIntent === 'wanted';
    const pending = current ? view.snapshot?.pending_interactions?.[0] : undefined;
    return { id: session.id, title: sessionDisplayTitle(session), viewOpen: openViews.includes(session.id),
      running: !!current && activeAttempt(view.snapshot), pendingInteraction: pending ? pending.request.kind.type === 'approval' ? 'approval' : pending.request.kind.type === 'review' ? 'plan-review' : 'question' : undefined,
      runningSubagentCount: 0,
      updatedAt: Date.parse(session.updated_at) || 0, observation: sessionObservation(state, session.id) };
  };
  const groupNodes: GroupNode[] = (bound ? catalog?.workspaces ?? [] : []).map(row => ({ key: row.id, workspaceId: row.id,
    cwd: row.displayPath, createdAt: undefined, label: row.displayName, expanded: true,
    containsCurrent: row.id === workspace,
    sessionCount: rows.filter(item => item.group === row.id).length, sessions: rows.filter(item => item.group === row.id).map(item => toNode(item.session)) }));
  return <>
    <WorkspaceBrowser wide={wide} expand={expand} groups={groupNodes} sessions={state.sessions.map(toNode)} selected={selected} closeView={closeView} closeAllViews={openViews.length ? closeAllViews : undefined}
      query={query} search={text => connected && search(text)} open={id => connected && openSession(id)} rename={(id, title) => edit('session', id, title)} fork={forkSession} remove={deleteSession}
      workspaceSettings={workspaceSettings} selectWorkspace={selectWorkspace} create={id => connected && createSession(id)} renameWorkspace={(id, title) => edit('workspace', id, title)} removeWorkspace={(id, title) => edit('remove', id, title)}
      addWorkspace={catalog?.picker.kind === 'configured' && bound ? () => setDialog({ kind: 'add', id: '', name: '' }) : undefined}
      refresh={() => { setReload(value => value + 1); search(query, offset); }} previous={connected && offset > 0 ? () => search(query, Math.max(0, offset - 32)) : undefined}
      next={connected && state.nextOffset != null ? () => search(query, state.nextOffset!) : undefined}
      notices={<>{hostError && <p role="status">{hostError}</p>}{!dialog && error && <p role="alert">{error}</p>}{catalog && !bound && <p role="status">This Workspace Host belongs to {catalog.endpoint}.</p>}</>} />
    {dialog && <Modal closeLabel="Close dialog" open title={dialog.kind === 'add' ? 'Add Workspace' : dialog.kind === 'remove' ? `Unregister ${dialog.name}?` : `Rename ${dialog.kind}`} onClose={() => { if (!busy) setDialog(undefined); }}>
      {error && <p role="alert">{error}</p>}
      {dialog.kind === 'add' ? <><p>Choose a location authorized by this Product Host.</p>{catalog?.picker.kind === 'configured' && catalog.picker.locations.map(location => <Button key={location.id} disabled={busy} onClick={() => void mutate(() => host.adoptWorkspace(location.id))}>{location.displayName}</Button>)}</>
          : dialog.kind === 'remove' ? <><p>Only the navigation registration is removed. Sessions, cwd, history, and running work remain untouched.</p><Button disabled={busy} onClick={() => void mutate(() => host.removeWorkspace(dialog.id), dialog.id)}>Unregister</Button></>
            : <form onSubmit={event => { event.preventDefault(); const current = navigation.capture(); void mutate(async () => {
              if (dialog.kind === 'workspace') await host.renameWorkspace(dialog.id, name);
              else { await client.renameSession(dialog.id, name); if (current()) await client.listSessions(offset, query, current); }
            }, dialog.kind === 'workspace'); }}><Input autoFocus disabled={busy} onKeyDown={event => { if (event.key === 'Enter' && (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229)) event.preventDefault(); }} aria-label="Name" value={name} onChange={event => setName(event.target.value)} /><Button type="submit" disabled={busy || !name.trim()}>Save name</Button></form>}
    </Modal>}
  </>;
}
