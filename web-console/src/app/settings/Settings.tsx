/* Copyright (c) 2026 DeepSeek. MIT. Adapted Settings shell; see PROVENANCE.md. */
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { EffectiveConfiguration, SourceSettings, SourceMutation, SourceScope, ConfigurationApplication } from '../../../../protocol/app-server/v14';
import { RpcFailure, isOutcomeUncertain, type AppServerClient } from '../../client/app-server';
import { Button } from '../../presentation/primitives/Button';
import { ResourceInventory } from './ResourceInventory';
import { NativeFacts } from '../components/NativeFacts';
import { Badge, Facts, SettingsCard } from '../../presentation/settings/SettingsContent';
import { CatalogEditor } from './CatalogEditor';
import { AgentEditor } from './AgentEditor';
import { Integrations } from './Integrations';
import { RootEditor, type RootSection } from './RootEditor';
import { RuntimeEditor } from './RuntimeEditor';
import type { SaveSource } from './controls';
import css from '../../presentation/settings/SettingsContent.module.css';
import { SettingsDrafts } from './drafts';
import { navigateTabs } from '../../presentation/primitives/tabs';
import { SettingsPanel } from '../../presentation/settings/SettingsRoot';
import type { ConnectionController } from '../../connection/controller';
import { ConnectionSettings } from './ConnectionSettings';

const sections = [
  ['overview', 'Overview', ''], ['general', 'General', 'Runtime'], ['catalog', 'Providers & Models', 'Runtime'], ['policies', 'Tool Policies', 'Runtime'],
  ['root-model', 'Model', 'Root Agent'], ['root-tools', 'Tools', 'Root Agent'], ['root-skills', 'Skill access', 'Root Agent'], ['root-plugins', 'Plugins', 'Root Agent'], ['root-agents', 'Agents & Workflows', 'Root Agent'],
  ['mcp', 'MCP', 'Resources'], ['python', 'Managed Python', 'Resources'], ['skills', 'Skills', 'Resources'], ['agents', 'Agents', 'Resources'], ['workflows', 'Workflows', 'Resources'], ['advanced', 'Diagnostics & source facts', 'Advanced'],
] as const;
type Section = typeof sections[number][0] | 'appearance' | 'connection';
function sourceRevision(source: SourceSettings, mutation: SourceMutation) {
  if (mutation.kind === 'config') return source[mutation.scope].revision;
  if (mutation.kind === 'mcp') return (mutation.scope === 'user' ? source.user_mcp : source.workspace_mcp).revision;
  return source.agents.find(agent => agent.scope === mutation.scope && agent.name === mutation.name)?.source.revision ?? source.absent_resource_revision;
}

function nativeDiagnostic(cause: unknown): string {
  if (!(cause instanceof RpcFailure)) return cause instanceof Error ? cause.message : String(cause);
  const data = cause.error.data;
  if (data?.kind === 'configuration_adoption') return data.rejection.status === 'failed' ? data.rejection.diagnostic : `Configuration adoption: ${data.rejection.status.replaceAll('_', ' ')}. Reread native state.`;
  return cause.error.message;
}

interface SettingsProps { client: AppServerClient; sessionId?: string; onClose?: () => void; theme?: 'light' | 'dark'; setTheme?: (theme: 'light' | 'dark') => void; connection?: ConnectionController; initialSection?: 'overview' | 'connection' }
function SettingsContent({ client, sessionId, onClose = () => {}, theme = 'light', setTheme, connection, section, setSection }: SettingsProps & { section: Section; setSection: (section: Section) => void }) {
  const transport = useSyncExternalStore(client.subscribe, client.getSnapshot);
  const target = sessionId ? transport.views[sessionId]?.target : undefined;
  const [scope, setScope] = useState<'effective' | SourceScope>('effective');
  const [source, setSource] = useState<SourceSettings>(), [effective, setEffective] = useState<EffectiveConfiguration>();
  const notified = sessionId ? transport.configuration?.[sessionId] : undefined;
  const application = !source?.application || (notified && BigInt(notified.version) > BigInt(source.application.version)) ? notified : source.application;
  const [message, setMessage] = useState(''), [error, setError] = useState(''), [busy, setBusy] = useState(false);
  const writing = useRef(false), epoch = useRef(0), readSequence = useRef(0);
  const needsRuntime = section !== 'connection' && section !== 'appearance';
  const read = useCallback(async () => {
    if (!sessionId || !needsRuntime) return { source: undefined, effective: undefined };
    const [authored, published] = await Promise.all([
      client.request({ method: 'configuration/sourcesRead', params: { session_id: sessionId } }, 'source_settings'),
      target ? client.request({ method: 'configuration/effective', params: { target } }, 'effective_configuration') : Promise.resolve(undefined),
    ]);
    return { source: authored.projection, effective: published?.projection };
  }, [client, sessionId, target, needsRuntime]);
  const refresh = useCallback(async (reportFailure = false) => {
    const at = epoch.current, sequence = ++readSequence.current;
    try {
      const next = await read();
      if (at === epoch.current && sequence === readSequence.current) { setSource(next.source); setEffective(next.effective); }
      return next;
    } catch (cause) {
      if (reportFailure && at === epoch.current && sequence === readSequence.current) setError(nativeDiagnostic(cause));
      throw cause;
    }
  }, [read]);
  useEffect(() => {
    const at = ++epoch.current, sequence = ++readSequence.current;
    void read().then(next => { if (epoch.current === at && sequence === readSequence.current) { setSource(next.source); setEffective(next.effective); } }).catch(cause => { if (epoch.current === at && sequence === readSequence.current) setError(nativeDiagnostic(cause)); });
    return () => { ++epoch.current; };
  }, [read, transport.generation]);
  useEffect(() => {
    if (notified) void refresh(true).catch(() => { /* Native application state remains visible even if source parsing fails. */ });
  }, [notified?.version, refresh]);
  const save: SaveSource = async (mutation, expected_revision) => {
    if (writing.current || transport.connection !== 'connected') return undefined;
    writing.current = true; setBusy(true); setError(''); setMessage('');
    const at = epoch.current;
    try {
      const result = await client.request({ method: 'configuration/sourceWrite', params: { session_id: sessionId!, expected_revision, mutation } }, 'source_settings');
      if (at !== epoch.current) return undefined;
      ++readSequence.current; // A read begun before this commit cannot replace its acknowledgement.
      setSource(result.projection);
      setMessage('Source saved. Native application continues automatically.');
      return sourceRevision(result.projection, mutation);
    } catch (cause) {
      if (at !== epoch.current) return undefined;
      const conflict = cause instanceof RpcFailure && cause.error.data?.kind === 'source_conflict';
      setError(conflict ? 'Conflict: source changed. Your draft and original revision are preserved. Review the current source before explicitly replacing it.' : isOutcomeUncertain(cause) ? 'Save outcome uncertain. Rereading authoritative sources; no write was replayed. Your draft is preserved.' : `Save failed: ${nativeDiagnostic(cause)}`);
      try { await refresh(); } catch { /* Retain draft and uncertainty until a successful explicit read. */ }
      return undefined;
    } finally { writing.current = false; setBusy(false); }
  };
  const applyIntent = async (adopt: boolean) => {
    if (!sessionId || writing.current || transport.connection !== 'connected') return;
    const candidate = application?.candidate;
    if (adopt && !candidate) return;
    writing.current = true; setBusy(true); setError(''); setMessage('');
    try {
      if (adopt && candidate) await client.request({ method: 'session/adoptConfiguration', params: {
        session_id: sessionId, candidate: candidate.identity, expected_binding: candidate.expected_binding,
      } }, 'configuration_application');
      else await client.request({ method: 'configuration/reconcile', params: { session_id: sessionId } }, 'configuration_application');
      await refresh();
    } catch (cause) {
      setError(isOutcomeUncertain(cause) ? 'Outcome uncertain. Reread native state; this action will not be replayed.' : nativeDiagnostic(cause));
      try { await refresh(); } catch { /* Retain the last observation until authority is available. */ }
    } finally { writing.current = false; setBusy(false); }
  };
  const frame = (children: import('react').ReactNode) => <SettingsPanel rows={[{ id: 'appearance', label: 'Appearance' }, ...(connection ? [{ id: 'connection', label: 'Connection' }] : []), ...sections.map(([id, label, group]) => ({ id, label, group }))]} activeId={section} onSelect={id => setSection(id as Section)} onClose={onClose}>{children}</SettingsPanel>;
  if (section === 'connection' && connection) return frame(<ConnectionSettings connection={connection} client={client} />);
  if (section === 'appearance') return frame(<section className={css.settings}><h2>Appearance</h2><p className={css.hint}>Presentation preferences are saved in this browser. Runtime configuration remains native.</p><label>Theme<select aria-label="Theme" value={theme} onChange={event => setTheme?.(event.target.value as 'light' | 'dark')}><option value="light">Light</option><option value="dark">Dark</option></select></label></section>);
  if (!source) return frame(<section className={css.settings} aria-label="Settings"><ConfigurationState application={application} busy={busy} adopt={() => void applyIntent(true)} retry={() => void applyIntent(false)} />{section === 'advanced' && <Button disabled={busy} onClick={() => void applyIntent(false)}>Rescan configuration files</Button>}{error ? <p role="alert">{error}</p> : <p role="status">{sessionId ? 'Loading configuration…' : 'Open a Session to inspect its native configuration.'}</p>}<Button onClick={() => void refresh(true).catch(() => { /* Current failures are reported inside the read fence. */ })}>Read current sources</Button></section>);
  const selected = scope === 'effective' ? undefined : source[scope];
  const models = [...new Set([...Object.keys(effective?.document.models ?? {}), ...Object.keys(source.user.authored?.models ?? {}), ...Object.keys(source.workspace.authored?.models ?? {})])];
  const roots = [`${source.user_resource_root}/skills`, `${source.workspace_resource_root}/skills`];
  return frame(<section className={css.settings} aria-label="Settings" aria-busy={busy}>
    <header><h2>{sections.find(([id]) => id === section)?.[1]}</h2><Button disabled={busy} onClick={() => void refresh(true).catch(() => { /* Current failures are reported inside the read fence. */ })}>Read current sources</Button></header>
    <div role="tablist" aria-label="Configuration scope" className={css.tabs} onKeyDown={navigateTabs}>{(['effective', 'user', 'workspace'] as const).map(value => <button key={value} role="tab" tabIndex={scope === value ? 0 : -1} aria-selected={scope === value} onClick={() => setScope(value)}>{value[0].toUpperCase() + value.slice(1)}</button>)}</div>
      <div className={css.content}>
        <ConfigurationState application={application} busy={busy} adopt={() => void applyIntent(true)} retry={() => void applyIntent(false)} />
        {effective?.admitted_attempt && effective.admitted_attempt.generation !== effective.generation && <p>The active Attempt retains its earlier captured configuration.</p>}
        {section === 'advanced' && <Button disabled={busy} onClick={() => void applyIntent(false)}>Rescan configuration files</Button>}

        {transport.connection !== 'connected' && <p role="status">Connection {transport.connection}. Values are the last native observation; drafts are retained. Reconnect rereads authority without replaying mutations.</p>}
        {message && <p role="status">{message}</p>}{error && <p className={css.error} role="alert">{error}</p>}

        {selected && <><p className={css.hint}>{scope === 'user' ? 'User' : 'Workspace'} source: {selected.path}<br />Revision: {selected.revision}</p>{selected.diagnostic && <p role="alert">{selected.diagnostic}</p>}<p className={css.hint}>Edits contain authored intent only. Remove a unit to expose the lower scope; save an empty selection to select none.</p></>}
        {scope === 'effective' ? effective ? <EffectiveView value={effective} source={source} section={section} /> : <p>Open a Session to inspect its published configuration.</p> : <fieldset disabled={busy} className={css.editor}>
          {section === 'catalog' && <CatalogEditor key={scope} document={selected?.authored ?? {}} scope={scope} revision={selected!.revision} save={save} />}
          {(section === 'general' || section === 'policies') && <RuntimeEditor key={`${scope}:${section}`} document={selected?.authored ?? {}} scope={scope} revision={selected!.revision} save={save} policyOnly={section === 'policies'} />}
          {section.startsWith('root-') && <RootEditor key={`${scope}:${section}`} document={selected?.authored ?? {}} scope={scope} revision={selected!.revision} save={save} section={section as RootSection} models={models} skillRoots={roots} />}
          {section === 'mcp' && <Integrations key={scope} source={source} scope={scope} save={save} />}
          {section === 'agents' && <AgentEditor key={scope} source={source} scope={scope} models={models} save={save} />}
          {['mcp', 'agents'].includes(section) && source.prospective_resources && <><h4>Current resource definitions</h4><ResourceInventory resources={source.prospective_resources} family={section} scope={scope} /></>}
          {['python', 'skills', 'workflows'].includes(section) && <><p>Resource definition root: {scope === 'user' ? source.user_resource_root : source.workspace_resource_root}. Root selection is edited separately.</p>{source.prospective_resources ? <><p>Current source analysis. External sources remain unprepared until admission.</p><ResourceInventory resources={source.prospective_resources} family={section} scope={scope} /></> : <p role="status">Source analysis unavailable: {source.prospective_diagnostic}</p>}</>}
          {section === 'overview' && <p>User &lt; Workspace resolution applies native semantic units. Save starts native application automatically. Existing Sessions explicitly adopt changed context. Admitted Attempts retain their captured configuration.</p>}
          {section === 'advanced' && <details><summary>Authored source facts (redacted)</summary><pre>{JSON.stringify(selected?.authored, null, 2)}</pre></details>}
        </fieldset>}
        <footer><details><summary>Source paths</summary><dl><dt>User config</dt><dd>{source.user.path}</dd><dt>User resources</dt><dd>{source.user_resource_root}</dd><dt>Workspace config</dt><dd>{source.workspace.path}</dd><dt>Workspace resources</dt><dd>{source.workspace_resource_root}</dd><dt>Runtime root</dt><dd>{source.runtime_root} · fixed for this process</dd></dl></details></footer>
      </div>
  </section>);
}
function ConfigurationState({ application, busy, adopt, retry }: { application?: ConfigurationApplication; busy: boolean; adopt: () => void; retry: () => void }) {
  if (!application) return <p>Configuration is current for this Session.</p>;
  const units = Object.values(application.units);
  const preparing = units.some(unit => unit?.status === 'preparing');
  const failed = units.some(unit => unit?.status === 'failed');
  const restart = units.some(unit => unit?.status === 'process_restart');
  return <section aria-label="Configuration application">
    {preparing && <p role="status">Preparing configuration… Current configuration remains available.</p>}
    {units.some(unit => unit?.status === 'applied') && <p>Applicable changes are applied for future independent Attempts.</p>}
    {application.candidate && <><p>Prepared context changes await this Session’s adoption.</p><Button disabled={busy} onClick={adopt}>Adopt prepared context</Button></>}
    {failed && <><p role="alert">Some configuration could not be applied. Effective configuration remains available.</p><Button disabled={busy} onClick={retry}>Retry application</Button></>}
    {restart && <p>Process restart required for pending process bindings.</p>}
    <details><summary>Application diagnostics</summary><NativeFacts value={application} /></details>
  </section>;
}

function EffectiveView({ value, source, section }: { value: EffectiveConfiguration; source: SourceSettings; section: Section }) {
  return <section aria-label="Effective configuration"><p>Read-only published configuration · generation {value.generation}</p>
    {section === 'overview' && <dl><dt>Root default model</dt><dd>{value.root_agent.model?.model}</dd><dt>Session explicit model</dt><dd>{value.session_model?.model ?? 'None'}</dd><dt>Effective model</dt><dd>{value.effective_model.effective.model}</dd><dt>Admitted Attempt</dt><dd>{value.admitted_attempt ? `${value.admitted_attempt.attempt} · generation ${value.admitted_attempt.generation} · ${value.admitted_attempt.model.effective.model}` : 'None'}</dd></dl>}
    {section === 'overview' && value.admitted_attempt && <details><summary>Admitted Attempt resources · frozen generation {value.admitted_attempt.generation}</summary><NativeFacts value={value.admitted_attempt.resources.main} /></details>}
    {section === 'catalog' && <><h4>Providers</h4><div className={css.rows}>{Object.entries(value.document.providers ?? {}).map(([id, provider]) => <SettingsCard key={id} title={id} meta={<Badge><Origin value={value} field={`providers.${id}`} /></Badge>}><Facts rows={[["Endpoint", provider.base_url], ["Credential source", provider.credential.type === 'environment' ? `Environment: ${provider.credential.variable}` : 'Literal secret (redacted)']]} /></SettingsCard>)}</div><h4>Models</h4><div className={css.rows}>{Object.entries(value.document.models ?? {}).map(([id, model]) => <SettingsCard key={id} title={id} meta={<Badge><Origin value={value} field={`models.${id}`} /></Badge>}><NativeFacts value={model} /></SettingsCard>)}</div></>}
    {section.startsWith('root-') && <><RootFacts value={value} section={section} /><details><summary>Prepared Root capabilities and native diagnostics</summary><NativeFacts value={value.resources.main} /></details>{section === 'root-skills' && <p>Prompt visibility, not filesystem access.<br />User Skill root: {source.user_resource_root}/skills<br />Workspace Skill root: {source.workspace_resource_root}/skills</p>}</>}
    {section === 'general' && <><Facts rows={[["Root identity", value.document.agent_id], ["Root description", value.root_agent.description], ["Instructions", value.root_agent.instructions], ["Approval mode", value.approval_mode]]} />{Object.entries({ Context: value.context, 'Model timeout': value.model_timeout, 'Tool deadline': value.tool_deadline, 'Subagent capacity': value.child_capacity, 'Authored environment': value.document.environment }).map(([title, fact]) => <SettingsCard key={title} title={title}><NativeFacts value={fact} /></SettingsCard>)}<SettingsCard title="App Server process policy"><p>Actual process bindings. Admission limits apply automatically; process drain policy waits for restart.</p><NativeFacts value={source.process_bindings} /></SettingsCard></>}
    {section === 'policies' && <table><thead><tr><th>Tool</th><th>Execution</th><th>Concurrency</th><th>Approval</th><th>Policy origin</th></tr></thead><tbody>{value.available_tools.map(tool => <tr key={tool.id}><td>{tool.name}</td><td>{JSON.stringify(tool.execution_policy)}</td><td>{JSON.stringify(tool.concurrency_policy)}</td><td>{JSON.stringify(tool.approval_policy)}</td><td><Origin value={value} field={tool.origin === 'builtin' ? `native_tools.${tool.name}` : 'mcp' in tool.origin ? `mcp_tool_policies.${tool.origin.mcp.server_id}` : 'python-native-policy'} /></td></tr>)}</tbody></table>}
    {['mcp', 'python', 'skills', 'agents', 'workflows'].includes(section) && <ResourceInventory resources={value.resources} family={section} />}
    {section === 'advanced' && <><h4>Diagnostics</h4>{value.resources.resource_diagnostics.map((diagnostic, index) => <p key={index}>{diagnostic.identity}: {diagnostic.reason} {diagnostic.file}</p>)}<details open><summary>Native provenance and source revisions</summary><NativeFacts value={{ provenance: value.provenance, source_revisions: value.source_revisions }} /></details></>}
  </section>;
}
function RootFacts({ value, section }: { value: EffectiveConfiguration; section: Section }) {
  const root = value.root_agent;
  const facts: [string, unknown, string][] = section === 'root-tools'
    ? [['Native Tools', root.tools?.builtin ?? [], 'agent.tools.builtin'], ...Object.entries(root.tools?.sources ?? {}).map(([id, selected]): [string, unknown, string] => [id, selected, `agent.tools.sources.${id}`])]
    : section === 'root-plugins' ? Object.entries(root.plugins ?? {}).map(([id, plugin]) => [id, plugin, `agent.plugins.${id}`])
    : section === 'root-skills' ? [['Skill prompt visibility', root.skills ?? [], 'agent.skills']]
    : section === 'root-agents' ? [['Named Agents', root.agents ?? [], 'agent.agents'], ['Workflows', root.workflows ?? [], 'agent.workflows']]
    : [['Model', root.model, 'agent.model'], ['Instructions', root.instructions, 'agent.instructions'], ['Project guidance', root.agents_md, 'agent.agents_md']];
  return <dl>{facts.map(([name, fact, field]) => <div key={field}><dt>{name}</dt><dd>{<NativeFacts value={fact} />} · <Origin value={value} field={field} /></dd></div>)}</dl>;
}
function Origin({ value, field }: { value: EffectiveConfiguration; field: string }) { const origin = value.provenance[field]; return <span>{origin ? origin.kind === 'builtin' ? 'Product default' : origin.kind === 'process' ? 'Process binding' : `${origin.kind}: ${origin.document}` : 'Native domain default'}</span>; }
export function Settings(props: SettingsProps) {
  const transport = useSyncExternalStore(props.client.subscribe, props.client.getSnapshot);
  const [section, setSection] = useState<Section>(props.initialSection ?? 'overview');
  return <SettingsDrafts key={`${transport.authorityRevision ?? 0}:${props.sessionId}`}><SettingsContent {...props} section={section} setSection={setSection} /></SettingsDrafts>;
}
