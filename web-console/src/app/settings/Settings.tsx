/* Copyright (c) 2026 DeepSeek. MIT. Adapted Settings shell; see PROVENANCE.md. */
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { SourceSettings, SourceMutation, SourceScope, SourceTarget } from '../../../../protocol/app-server/v17';
import { RpcFailure, isOutcomeUncertain, type AppServerClient } from '../../client/app-server';
import { Button } from '../../presentation/primitives/Button';
import { ResourceInventory } from './ResourceInventory';
import { CatalogEditor } from './CatalogEditor';
import { AgentEditor } from './AgentEditor';
import { Integrations } from './Integrations';
import { RootEditor, type RootSection } from './RootEditor';
import { RuntimeEditor } from './RuntimeEditor';
import { UnitForm, type SaveSource } from './controls';
import css from '../../presentation/settings/SettingsContent.module.css';
import { DraftContext, SourceContext, type UnitDraft } from './drafts';
import { SettingsPanel } from '../../presentation/settings/SettingsRoot';
import type { ConnectionController } from '../../connection/controller';
import { ConnectionSettings } from './ConnectionSettings';
import { WorkspaceHostError, type ProductHostWorkspaces, type WorkspaceCatalog, type WorkspaceConfigurationOperation } from '../../workspaces/host';

const sections = [
  ['overview', 'Overview', 'General'], ['general', 'General', 'General'], ['catalog', 'Providers & Models', 'Models'],
  ['root-model', 'Default model', 'Models'], ['policies', 'Tool Policies', 'Agents & Tools'],
  ['root-tools', 'Tools', 'Agents & Tools'], ['root-skills', 'Skill access', 'Agents & Tools'],
  ['root-plugins', 'Plugins', 'Agents & Tools'], ['root-agents', 'Agents & Workflows', 'Agents & Tools'],
  ['agents', 'Agents', 'Agents & Tools'], ['mcp', 'MCP', 'Integrations'], ['python', 'Managed Python', 'Integrations'],
  ['skills', 'Skills', 'Integrations'], ['workflows', 'Workflows', 'Integrations'], ['advanced', 'Server & source diagnostics', 'Advanced'],
] as const;
type Section = typeof sections[number][0] | 'appearance' | 'connection';
const draftStores = new WeakMap<AppServerClient, Map<string, Map<string, UnitDraft>>>();
interface SettingsProps {
  client: AppServerClient; workspaceId?: string; host?: ProductHostWorkspaces; onClose?: () => void;
  theme?: 'light' | 'dark'; setTheme?: (theme: 'light' | 'dark') => void;
  connection?: ConnectionController; initialSection?: 'overview' | 'connection';
}
/** The native application scope this source target publishes under, exactly as
 * `SourceTarget::application_scope` names it. Application versions are u64
 * counters comparable only inside one scope, authority and connection lifetime. */
function applicationScope(target: SourceTarget) {
  return target.kind === 'user' ? 'source:user' : `source:workspace:${target.directory}`;
}
function sourceRevision(source: SourceSettings, mutation: SourceMutation) {
  const scope = source.target.kind;
  if ((mutation.kind === 'config' || mutation.kind === 'repair_config')) return source[scope]!.revision;
  if (mutation.kind === 'mcp') return (scope === 'user' ? source.user_mcp : source.workspace_mcp)!.revision;
  return source.agents.find(agent => agent.scope === scope && agent.name === mutation.name)?.source.revision ?? source.absent_resource_revision;
}
export function Settings({ client, workspaceId: initialWorkspace, host, onClose = () => {}, theme = 'light', setTheme, connection, initialSection }: SettingsProps) {
  const transport = useSyncExternalStore(client.subscribe, client.getSnapshot);
  const [workspaceId, setWorkspaceId] = useState(initialWorkspace);
  useEffect(() => { setWorkspaceId(initialWorkspace); }, [initialWorkspace]);
  const [catalog, setCatalog] = useState<WorkspaceCatalog>();
  const [section, setSection] = useState<Section>(initialSection ?? 'overview');
  const [source, setSource] = useState<SourceSettings>();
  const [error, setError] = useState(''), [message, setMessage] = useState(''), [busy, setBusy] = useState(false);
  const [targetValid, setTargetValid] = useState(false);
  // Separate facts, never one counter: `epoch` fences target, authority and
  // connection lifetime; `reads` orders authoritative reads; `accepted` counts
  // adopted projections so a mutation acknowledgement never poses as read order.
  const epoch = useRef(0), reads = useRef(0), accepted = useRef(0), writing = useRef<number | undefined>(undefined);
  // Level-triggered observation state. `observation` is the latest accepted whole
  // projection; `publications` mirrors the client's per-scope application map;
  // `commits` counts acknowledged source writes whose committed revision no
  // accepted projection has been read after yet; `converging` owns the single
  // convergence worker by the epoch that started it.
  const observation = useRef<SourceSettings | undefined>(undefined), converging = useRef<number | undefined>(undefined);
  const publications = useRef(transport.configuration), commits = useRef(0), observedCommits = useRef(0);
  publications.current = transport.configuration;
  const endpoint = transport.endpoint ?? '';
  const identity = JSON.stringify([endpoint, transport.authorityRevision, workspaceId ?? null]);
  let stores = draftStores.get(client);
  if (!stores) { stores = new Map(); draftStores.set(client, stores); }
  const draftKey = identity + ':' + section;
  let drafts = stores.get(draftKey);
  if (!drafts) { drafts = new Map(); stores.set(draftKey, drafts); }
  const request = useCallback(async (operation: WorkspaceConfigurationOperation) => {
    if (workspaceId !== undefined) {
      if (!host?.configureWorkspace) throw new Error('Workspace Settings requires an authorized Product Host connection.');
      return host.configureWorkspace(workspaceId, endpoint, operation);
    }
    const target = { kind: 'user' as const };
    if (operation.kind === 'write') return (await client.request({ method: 'configuration/sourceWrite', params: { target, expected_revision: operation.expected_revision, mutation: operation.mutation } }, 'source_settings')).projection;
    if (operation.kind === 'reconcile') await client.request({ method: 'configuration/reconcile', params: { target } }, 'configuration_application');
    return (await client.request({ method: 'configuration/sourcesRead', params: { target } }, 'source_settings')).projection;
  }, [client, endpoint, host, workspaceId]);
  /** Adopt one whole authoritative projection. Only `configuration/sourcesRead`
   * results reach here, ordered by `reads`: a source-write acknowledgement
   * confirms one authoring mutation and supplies its committed revision, but is
   * not an application observation and never replaces the read model. */
  const accept = useCallback((next: SourceSettings) => {
    observation.current = next; ++accepted.current;
    setSource(next); setTargetValid(true);
  }, []);
  /** One authoritative read. Resolves true only when this read's own projection
   * was adopted; a newer outstanding read wins instead. An adopted read was
   * issued after every commit acknowledged before it started, so it observes
   * them. Read failures are reported only when no projection accepted meanwhile
   * already answered them. */
  const refresh = useCallback(async () => {
    const at = epoch.current, read = ++reads.current, settled = accepted.current, afterCommits = commits.current;
    try {
      const next = await request({ kind: 'read' });
      if (at !== epoch.current || read !== reads.current) return false;
      if (afterCommits > observedCommits.current) observedCommits.current = afterCommits;
      accept(next); return true;
    } catch (cause) {
      if (at === epoch.current && read === reads.current && settled === accepted.current) { setError(String(cause)); setTargetValid(false); }
      throw cause;
    }
  }, [accept, request]);
  /** The outstanding publication obligation: an application version published
   * for this target's scope that the accepted projection has not reached yet.
   * Level, not edge — it survives reads, acknowledgements and worker restarts
   * until an authoritative read carries at least that version for the scope.
   * With no projection this lifetime, any start owes the first authoritative
   * read (0n), which also retries a failed initial read on later triggers. */
  const obligation = useCallback(() => {
    const held = observation.current, published = publications.current ?? {};
    if (!held) return 0n;
    const scope = applicationScope(held.target), publication = published[scope];
    if (!publication) return undefined;
    const settled = held.application?.scope === scope ? held.application.version : undefined;
    return settled === undefined || BigInt(settled) < BigInt(publication.version) ? BigInt(publication.version) : undefined;
  }, []);
  /** The single convergence worker for the current lifetime. Each pass observes
   * one real outstanding obligation with one authoritative read, then
   * re-evaluates the level: a publication that arrived while the read was
   * outstanding is still owed and drives exactly one more bounded read. A
   * newer one-shot read (explicit refresh, save recovery) supersedes the
   * worker's and ends its pass; the effects re-arm it if an obligation remains.
   * No timers, no polling, no write replay. */
  const converge = useCallback(async () => {
    if (converging.current !== undefined) return;
    const owner = epoch.current;
    converging.current = owner;
    try {
      for (;;) {
        const at = epoch.current, required = obligation();
        if (required === undefined && commits.current === observedCommits.current) break;
        const adopted = await refresh();
        if (at !== epoch.current || !adopted) break;
        if (required === undefined) continue;
        const remaining = obligation();
        if (remaining === undefined || remaining > required) continue;
        // The read was issued after `required` was published, so settling below
        // it is native evidence missing, not a reason to spin. Absent application
        // data is not measurable; a measurably stale application is reported.
        const scope = applicationScope(observation.current!.target);
        const settled = observation.current?.application?.scope === scope ? observation.current.application.version : undefined;
        if (settled === undefined) break;
        setError(`Native ${scope} published application version ${required}, but the authoritative read issued after it settled at ${settled}.`);
        break;
      }
    } catch { /* refresh already owns reporting this failure; a later publication, refresh or reconnect may retry. */ }
    finally { if (converging.current === owner) converging.current = undefined; }
  }, [refresh, obligation]);
  useEffect(() => {
    ++epoch.current; observation.current = undefined; converging.current = undefined;
    commits.current = 0; observedCommits.current = 0;
    setSource(undefined); setTargetValid(false); setBusy(false); setError(''); setMessage('');
    if (transport.connection === 'connected') void converge();
    return () => { ++epoch.current; };
  }, [identity, transport.generation, transport.connection, converge]);
  useEffect(() => {
    let current = true;
    if (host) void host.listWorkspaces().then(value => { if (current) setCatalog(value); }).catch(() => { if (current) setCatalog(undefined); });
    return () => { current = false; };
  }, [host, identity]);
  // Native source publications observed on this connection. `owed` is a level,
  // not an edge: until this target's own projection carries at least the version
  // published for its scope, the observation obligation stands — an older
  // acknowledgement landing in between cannot discharge or cancel it.
  const publicationsList = Object.entries(transport.configuration ?? {}).filter(([scope]) => scope.startsWith('source:'));
  const observed = publicationsList.map(([scope, value]) => `${scope}=${value.version}`).join(' ');
  const published = source && publicationsList.find(([scope]) => scope === applicationScope(source.target))?.[1].version;
  const settled = source?.application?.version;
  const owed = published !== undefined && (settled === undefined || BigInt(settled) < BigInt(published));
  useEffect(() => { if (observed) void converge(); }, [observed, converge]);
  useEffect(() => { if (owed) void converge(); }, [owed, settled, converge]);
  const save: SaveSource = async (mutation, expected_revision) => {
    if (writing.current === epoch.current || !targetValid || transport.connection !== 'connected') return undefined;
    const at = epoch.current;
    writing.current = at; setBusy(true); setError(''); setMessage('');
    try {
      const next = await request({ kind: 'write', expected_revision, mutation });
      if (at !== epoch.current) return undefined;
      // The acknowledgement confirms this one mutation and supplies its
      // committed revision; it is not adopted as a projection. Success settles
      // only after an authoritative read issued after this commit is adopted,
      // so every draft's CAS base reads from a projection at least as current
      // as the acknowledged write — never from the acknowledgement itself. A
      // failed read does not undo the committed write: refresh already
      // reported it and the outcome still stands.
      const commit = ++commits.current;
      const committed = sourceRevision(next, mutation);
      while (at === epoch.current && observedCommits.current < commit) {
        try { await refresh(); } catch { break; }
      }
      if (at !== epoch.current) return undefined;
      setMessage('Source saved. Native coordination owns application.');
      void converge();
      return committed;
    } catch (cause) {
      if (at !== epoch.current) return undefined;
      const conflict = (cause instanceof RpcFailure && cause.error.data?.kind === 'source_conflict') || (cause instanceof WorkspaceHostError && cause.kind === 'source_conflict');
      const uncertain = isOutcomeUncertain(cause) || (cause instanceof WorkspaceHostError && cause.uncertain);
      setError(conflict ? 'Source changed. Your draft and base revision are preserved.' : uncertain ? 'Save outcome uncertain. Rereading authority without replaying the write.' : String(cause));
      try { await refresh(); } catch { /* Keep draft and invalid target until an authoritative read succeeds. */ }
      return undefined;
    } finally { if (writing.current === at) writing.current = undefined; if (at === epoch.current) setBusy(false); }
  };
  const scope: SourceScope = workspaceId === undefined ? 'user' : 'workspace';
  const selected = source?.[scope];
  const models = Object.keys(source?.resolved?.models ?? source?.user.authored?.models ?? {});
  const roots = [source?.user_resource_root ? source.user_resource_root + '/skills' : '', source?.workspace_resource_root ? source.workspace_resource_root + '/skills' : ''];
  const editor = selected && <fieldset disabled={busy || !targetValid || transport.connection !== 'connected'} className={css.editor}>
    {section === 'catalog' && <CatalogEditor document={selected.authored ?? {}} scope={scope} revision={selected.revision} save={save} />}
    {(section === 'general' || section === 'policies') && <RuntimeEditor document={selected.authored ?? {}} scope={scope} revision={selected.revision} save={save} policyOnly={section === 'policies'} processPolicyImpacts={source!.process_policy_impacts} />}
    {section.startsWith('root-') && <RootEditor document={selected.authored ?? {}} scope={scope} revision={selected.revision} save={save} section={section as RootSection} models={models} skillRoots={roots} />}
    {section === 'mcp' && <Integrations source={source!} scope={scope} save={save} />}
    {section === 'agents' && <AgentEditor source={source!} scope={scope} models={models} save={save} />}
    {['mcp', 'agents', 'python', 'skills', 'workflows'].includes(section) && source?.prospective_resources && <ResourceInventory resources={source.prospective_resources} family={section} scope={scope} />}
  </fieldset>;
  return <SettingsPanel rows={[{ id: 'appearance', label: 'Appearance' }, ...(connection ? [{ id: 'connection', label: 'Connection' }] : []), ...sections.map(([id, label, group]) => ({ id, label, group }))]} activeId={section} onSelect={id => setSection(id as Section)} onClose={onClose}>
    {section === 'connection' && connection ? <ConnectionSettings connection={connection} client={client} /> : section === 'appearance' ?
      <section><h2>Appearance</h2><label>Theme<select aria-label="Theme" value={theme} onChange={event => setTheme?.(event.target.value as 'light' | 'dark')}><option value="light">Light</option><option value="dark">Dark</option></select></label></section> :
      <section className={css.settings} aria-label="Settings" aria-busy={busy}>
        <h2>{scope === 'user' ? 'User Settings' : 'Workspace Settings'}</h2>
        <label>Configuration owner<select aria-label="Configuration owner" value={workspaceId ?? ''} disabled={busy} onChange={event => setWorkspaceId(event.target.value || undefined)}>
          <option value="">User</option>
          {workspaceId && !catalog?.workspaces.some(row => row.id === workspaceId) && <option value={workspaceId}>Unavailable Workspace</option>}
          {catalog?.endpoint === endpoint && catalog.workspaces.map(row => <option key={row.id} value={row.id}>{row.displayName}</option>)}
        </select></label>
        <Button disabled={busy || transport.connection !== 'connected'} onClick={() => void refresh().catch(() => {})}>Read current sources</Button>
        {error && <p role="alert">{error}</p>}{message && <p role="status">{message}</p>}
        {!source && <p role="status">Loading source authority…</p>}
        {selected && <p>{selected.path} · Revision: {selected.revision}</p>}
        {selected?.diagnostic && <p role="alert">{selected.diagnostic}</p>}
        {source?.prospective_diagnostic && <p role="status">{source.prospective_diagnostic}</p>}
        {scope === 'workspace' && <p>Remove an override to reset to the global default. An explicit empty selection means none.</p>}
        <h3>{sections.find(([id]) => id === section)?.[1]}</h3>
        <SourceContext value={source}><DraftContext value={drafts}><div key={draftKey}>{editor}
          {selected?.diagnostic && !selected.authored && <UnitForm title="Repair malformed source" initial="" revision={selected.revision} save={save} removable={false}
            mutation={document => ({ kind: 'repair_config', document: document ?? '' })}>
            {(value, change) => <label>Replacement TOML<textarea value={value} onChange={event => change(event.target.value)} /></label>}
          </UnitForm>}
        </div></DraftContext></SourceContext>
        {section === 'overview' && <p>Definitions and defaults belong to this source. Session selections and explicit adoption belong to each Session.</p>}
        {section === 'advanced' && source && <>
          <h3>Process bindings</h3><pre>{JSON.stringify(source.process_bindings, null, 2)}</pre>
          {source.application?.units.process_bindings?.status === 'process_restart' && <p role="status">Saved desired values differ from the current process binding. Restart required.</p>}
          {source.application?.units.process_bindings?.status === 'applied' && <p role="status">Saved process policy is active.</p>}
          <details><summary>Resolved preview — source resolution only</summary><pre>{JSON.stringify({ resolved: source.resolved, provenance: source.provenance }, null, 2)}</pre></details>
          <details><summary>Source and application diagnostics</summary><pre>{JSON.stringify(source, null, 2)}</pre></details>
          <Button disabled={busy || !targetValid} onClick={() => { const at = epoch.current; void request({ kind: 'reconcile' }).then(() => { if (at === epoch.current) return refresh(); }).catch(cause => { if (at === epoch.current) setError(String(cause)); }); }}>Rescan configuration files</Button>
        </>}
      </section>}
  </SettingsPanel>;
}
