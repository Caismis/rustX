/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-workspace browser/rows/picker; see PROVENANCE.md. */
import { useEffect, useState } from 'react';
import type { AppServerClient, ClientView, SessionView } from '../client/app-server';
import { activeAttempt } from '../bindings/projection';
import { NavigationEpoch } from '../app/commands/native';
import { Button } from '../presentation/primitives/Button';
import { Input } from '../presentation/primitives/Input';
import { Dialog } from '../presentation/primitives/Dialog';
import { StateDot } from '../presentation/primitives/StateDot';
import { sameEndpoint } from './endpoint';
import type { ProductHostWorkspaces, WorkspaceCatalog, SessionLocation } from './host';
import css from './WorkspaceNavigation.module.css';

/** Activity requires a current attachment; cached snapshots cannot claim execution. */
export function sessionObservation(view: SessionView | undefined, connected: boolean, residency?: string) {
  if (!connected) return 'Observation stale / disconnected';
  if (view?.attachment === 'detached') return `Durable · detached · ${residency?.toLowerCase() ?? 'runtime not observed'} (list observation)`;
  if (!view) return residency ? `Durable · ${residency.toLowerCase()} (list observation)` : 'Durable · runtime not observed';
  if (view.attachment === 'unloaded') return 'Durable · unloaded';
  if (view.attachmentIntent !== 'wanted') return 'Detached / observation stale';
  if (view.attachment !== 'attached' || !view.target) return `Observation ${view.attachment}`;
  if (view.snapshot?.inbound.pending?.length) return `Pending inbound · ${view.snapshot.inbound.pending.length}`;
  if (activeAttempt(view.snapshot)) return 'Running';
  return 'Loaded / attached';
}
export function WorkspaceNavigation({ host, client, state, endpoint, navigation, workspace, selected, selectWorkspace, openSession, createSession, forkSession, deleteSession, creating, metadataChanged }: {
  host: ProductHostWorkspaces; client: AppServerClient; state: ClientView; endpoint: string; navigation: NavigationEpoch;
  creating: boolean; metadataChanged: (removed?: string) => void;
  workspace?: string; selected?: string; selectWorkspace: (id?: string) => void;
  openSession: (id: string) => void; createSession: (id: string) => void; forkSession: (id: string) => void; deleteSession: (id: string) => void;
}) {
  const [catalog, setCatalog] = useState<WorkspaceCatalog>();
  const [groups, setGroups] = useState<SessionLocation[]>([]);
  const [grouped, setGrouped] = useState(true), [collapsed, setCollapsed] = useState<string[]>([]);
  const [query, setQuery] = useState(''), [offset, setOffset] = useState(0);
  const [hostError, setHostError] = useState('');
  const [error, setError] = useState(''), [busy, setBusy] = useState(false), [reload, setReload] = useState(0);
  const [dialog, setDialog] = useState<{ kind: 'workspace' | 'session' | 'remove' | 'add' | 'settings'; id: string; name: string }>();
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
    setGroups([]);
    if (bound && connected) void host.classifyLocations(state.sessions.map(row => row.cwd), route).then(value => { if (alive) setGroups(value); }, cause => { if (alive) setError(String(cause)); });
    return () => { alive = false; };
  }, [host, catalog, state.sessions, route, connected, bound]);
  const search = (text: string, page = 0) => {
    navigation.invalidate(); const current = navigation.capture();
    setQuery(text); setOffset(page); setError('');
    void client.listSessions(page, text, current).catch(cause => { if (current()) setError(String(cause)); });
  };
  const edit = (kind: 'workspace' | 'session' | 'remove', id: string, title: string) => { setName(title); setDialog({ kind, id, name: title }); };
  const mutate = async (action: () => Promise<unknown>, metadata: boolean | string = true) => {
    const current = navigation.capture();
    if (busy) return;
    setBusy(true); setError('');
    try { await action(); setReload(value => value + 1); setDialog(undefined); if (metadata && current()) metadataChanged(typeof metadata === 'string' ? metadata : undefined); }
    catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  const rows = state.sessions.map((session, index) => ({ session, location: groups[index], group: groups[index]?.authorized ? groups[index].workspaceId ?? null : null }));
  const trustView = selected && rows.some(row => row.session.id === selected && row.group === workspace) ? state.views[selected] : undefined;
  const trust = connected && trustView?.attachment === 'attached' && trustView.attachmentIntent === 'wanted' ? trustView.projectTrusted : undefined;
  const trustLabel = trust === true ? 'Trusted project source' : trust === false ? 'Untrusted project source · project resources inactive on cold resolution' : 'Project trust unknown · open a Session to read native status';
  const sessionRows = (group?: string | null) => rows.filter(row => group === undefined || row.group === group).map(({ session, location }) => <div className={css.sessionRow} key={session.id} data-selected={selected === session.id}>
    <StateDot state={connected && state.views[session.id]?.attachment === 'attached' && state.views[session.id]?.attachmentIntent === 'wanted' ? state.views[session.id]?.snapshot?.inbound.pending?.length ? 'warning' : activeAttempt(state.views[session.id]?.snapshot) ? 'ongoing' : 'idle' : 'idle'} />
    <button className={`session-open ${css.open}`} aria-label={`Open ${session.name ?? session.id}`} aria-current={selected === session.id ? 'page' : undefined} disabled={!connected} onClick={() => openSession(session.id)}>
      <strong>{session.name ?? session.preview ?? session.id}</strong><small>{location?.authorized ? location.workspaceId ? 'Host authorized · registered' : 'Host authorized · ungrouped' : location ? 'Not authorized by this Product Host' : 'Host authorization not observed'}</small><small>{sessionObservation(state.views[session.id], connected, state.sessionResidencies?.[session.id])}</small>
    </button>
    <details className={css.actions}><summary aria-label={`Actions ${session.name ?? session.id}`}>···</summary><div>
      <Button size="sm" disabled={!connected} onClick={() => edit('session', session.id, session.name ?? '')}>Rename Session</Button>
      <Button size="sm" disabled={!connected} onClick={() => forkSession(session.id)}>Fork Session</Button>
      <Button size="sm" aria-label={`Delete ${session.name ?? session.id}`} disabled={!connected} onClick={() => deleteSession(session.id)}>Delete</Button>
    </div></details>
  </div>);
  return <section className={css.root} aria-label="Workspaces and Sessions">
    <div className="section-head"><h2>Workspaces</h2><Button size="sm" onClick={() => setGrouped(value => !value)}>{grouped ? 'Flat view' : 'Grouped view'}</Button></div>
    <label>Choose Workspace<select aria-label="Choose Workspace" value={workspace ?? ''} disabled={!bound} onChange={event => selectWorkspace(event.target.value || undefined)}>
      <option value="">Select authorized Workspace</option>{catalog?.workspaces.map(row => <option key={row.id} value={row.id}>{row.displayName}</option>)}
    </select></label>
    <div className="row"><Button variant="outline" disabled={!connected || !bound || !workspace || busy || creating} onClick={() => workspace && createSession(workspace)}>Create Session</Button>
      {catalog?.picker.kind === 'configured' && <Button disabled={busy || !bound} onClick={() => setDialog({ kind: 'add', id: '', name: '' })}>Add Workspace</Button>}</div>
    {catalog?.picker.kind === 'unavailable' && <small className="muted">{catalog.picker.reason}</small>}
    {catalog && !bound && <p role="status">This Workspace Host belongs to {catalog.endpoint}. Connect to that process to use its registrations.</p>}
    {workspace && <div className={css.trust}><small>Host authorization: registered</small><small>{trustLabel}</small>
      <Button size="sm" disabled={trust !== true} onClick={() => setDialog({ kind: 'settings', id: workspace, name: '' })}>Workspace settings</Button></div>}
    <Input aria-label="Search Session metadata" placeholder="Search Session metadata" value={query} disabled={!connected} onChange={event => search(event.target.value)} />
    <div className="row"><Button size="sm" disabled={!connected} onClick={() => { setReload(value => value + 1); search(query, offset); }}>Refresh list</Button><small className="muted">32 summaries per page</small></div>
    {hostError && <p role="status">{hostError}</p>}
    {error && <p role="alert">{error}</p>}
    <div className={css.list}>
      {grouped && !query ? <>{bound && catalog?.workspaces.map((row, index) => <div key={row.id} className={css.group}>
        <div className={css.workspaceRow} data-selected={workspace === row.id}>
          <button aria-label={`Toggle ${row.displayName}`} aria-expanded={!collapsed.includes(row.id)} onClick={() => setCollapsed(value => value.includes(row.id) ? value.filter(id => id !== row.id) : [...value, row.id])}>{collapsed.includes(row.id) ? '▸' : '▾'}</button>
          <button className={css.title} aria-label={`Select Workspace ${row.displayName}`} title={row.displayPath} onClick={() => selectWorkspace(row.id)}>{row.displayName}</button>
          <details className={css.actions}><summary aria-label={`Workspace actions ${row.displayName}`}>···</summary><div>
            <Button size="sm" disabled={busy} onClick={() => edit('workspace', row.id, row.displayName)}>Rename Workspace</Button>
            <Button size="sm" disabled={busy || index === 0} onClick={() => void mutate(() => host.reorderWorkspace(row.id, catalog.workspaces[index - 1].id))}>Move up</Button>
            <Button size="sm" disabled={busy || index === catalog.workspaces.length - 1} onClick={() => void mutate(() => host.reorderWorkspace(row.id, catalog.workspaces[index + 2]?.id))}>Move down</Button>
            <Button size="sm" disabled={busy} onClick={() => edit('remove', row.id, row.displayName)}>Unregister Workspace</Button>
          </div></details>
        </div>{!collapsed.includes(row.id) && sessionRows(row.id)}
      </div>)}<div className={css.group}><h3>Ungrouped Sessions</h3>{sessionRows(null)}</div></> : sessionRows()}
    </div>
    <div className="row"><Button size="sm" disabled={!connected || offset === 0} onClick={() => search(query, Math.max(0, offset - 32))}>Previous</Button><Button size="sm" disabled={!connected || state.nextOffset == null} onClick={() => search(query, state.nextOffset!)}>Next</Button></div>
    {dialog && <Dialog open title={dialog.kind === 'add' ? 'Add Workspace' : dialog.kind === 'remove' ? `Unregister ${dialog.name}?` : dialog.kind === 'settings' ? 'Workspace source status' : `Rename ${dialog.kind}`} onClose={() => setDialog(undefined)}>
      {dialog.kind === 'settings' ? <p>Native current source trust: {trustLabel}. Loaded resource activation belongs to its admitted native generation; inspect Resources for effective state.</p>
        : dialog.kind === 'add' ? <><p>Choose a location authorized by this Product Host.</p>{catalog?.picker.kind === 'configured' && catalog.picker.locations.map(location => <Button key={location.id} disabled={busy} onClick={() => void mutate(() => host.adoptWorkspace(location.id))}>{location.displayName}</Button>)}</>
          : dialog.kind === 'remove' ? <><p>Only the navigation registration is removed. Sessions, cwd, history, project trust, and running work remain untouched.</p><Button disabled={busy} onClick={() => void mutate(() => host.removeWorkspace(dialog.id), dialog.id)}>Unregister</Button></>
            : <form onSubmit={event => { event.preventDefault(); const current = navigation.capture(); void mutate(async () => {
              if (dialog.kind === 'workspace') await host.renameWorkspace(dialog.id, name);
              else { await client.request({ method: 'session/name', params: { session_id: dialog.id, name } }, 'session'); if (current()) await client.listSessions(offset, query, current); }
            }, dialog.kind === 'workspace'); }}><Input autoFocus aria-label="Name" value={name} onChange={event => setName(event.target.value)} /><Button type="submit" disabled={busy || !name.trim()}>Save name</Button></form>}
    </Dialog>}
  </section>;
}
