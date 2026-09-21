/* Copyright (c) 2026 DeepSeek. MIT. Adapted Settings shell; see PROVENANCE.md. */
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { SourceSettings, SourceMutation, SourceScope } from '../../../../protocol/app-server/v17';
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
  const epoch = useRef(0), sequence = useRef(0), writing = useRef<number | undefined>(undefined);
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
  const refresh = useCallback(async () => {
    const at = epoch.current, read = ++sequence.current;
    try {
      const next = await request({ kind: 'read' });
      if (at === epoch.current && read === sequence.current) { setSource(next); setTargetValid(true); }
    } catch (cause) {
      if (at === epoch.current && read === sequence.current) { setError(String(cause)); setTargetValid(false); }
      throw cause;
    }
  }, [request]);
  useEffect(() => {
    ++epoch.current; setSource(undefined); setTargetValid(false); setBusy(false); setError(''); setMessage('');
    if (transport.connection === 'connected') void refresh().catch(() => {});
    return () => { ++epoch.current; };
  }, [identity, transport.generation, transport.connection, refresh]);
  useEffect(() => {
    let current = true;
    if (host) void host.listWorkspaces().then(value => { if (current) setCatalog(value); }).catch(() => { if (current) setCatalog(undefined); });
    return () => { current = false; };
  }, [host, identity]);
  const notifications = Object.values(transport.configuration ?? {}).filter(value => value.scope.startsWith('source:')).map(value => value.version).join(':');
  useEffect(() => { if (notifications) void refresh().catch(() => {}); }, [notifications, refresh]);
  const save: SaveSource = async (mutation, expected_revision) => {
    if (writing.current === epoch.current || !targetValid || transport.connection !== 'connected') return undefined;
    const at = epoch.current;
    writing.current = at; setBusy(true); setError(''); setMessage('');
    try {
      const next = await request({ kind: 'write', expected_revision, mutation });
      if (at !== epoch.current) return undefined;
      ++sequence.current; setSource(next); setMessage('Source saved. Native coordination owns application.');
      return sourceRevision(next, mutation);
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
