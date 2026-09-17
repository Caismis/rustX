/* Copyright (c) 2026 DeepSeek. MIT. Adapted Settings shell; see PROVENANCE.md. */
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { CapabilityInspection, EffectiveConfiguration, SourceSettings, SourceMutation, SourceScope } from '../../../../protocol/app-server/v6';
import { RpcFailure, isOutcomeUncertain, type AppServerClient } from '../../client/app-server';
import { Button } from '../../presentation/primitives/Button';
import { CatalogEditor } from './CatalogEditor';
import { AgentEditor } from './AgentEditor';
import { Integrations } from './Integrations';
import { RootEditor, type RootSection } from './RootEditor';
import { RuntimeEditor } from './RuntimeEditor';
import type { SaveSource } from './controls';
import css from './Settings.module.css';
import { SettingsPanel } from '../../presentation/settings/SettingsRoot';

const sections = [
  ['overview', 'Overview', ''], ['general', 'General', 'Runtime'], ['catalog', 'Providers & Models', 'Runtime'], ['policies', 'Tool Policies', 'Runtime'],
  ['root-model', 'Model', 'Root Agent'], ['root-tools', 'Tools', 'Root Agent'], ['root-skills', 'Skill access', 'Root Agent'], ['root-plugins', 'Plugins', 'Root Agent'], ['root-agents', 'Agents & Workflows', 'Root Agent'],
  ['mcp', 'MCP', 'Resources'], ['python', 'Managed Python', 'Resources'], ['skills', 'Skills', 'Resources'], ['agents', 'Agents', 'Resources'], ['workflows', 'Workflows', 'Resources'], ['advanced', 'Diagnostics & source facts', 'Advanced'],
] as const;
type Section = typeof sections[number][0] | 'appearance';
function sourceRevision(source: SourceSettings, mutation: SourceMutation) {
  if (mutation.kind === 'config') return source[mutation.scope].revision;
  if (mutation.kind === 'mcp') return (mutation.scope === 'user' ? source.user_mcp : source.workspace_mcp).revision;
  return source.agents.find(agent => agent.scope === mutation.scope && agent.name === mutation.name)?.source.revision ?? source.absent_resource_revision;
}

function nativeDiagnostic(cause: unknown): string {
  if (!(cause instanceof RpcFailure)) return cause instanceof Error ? cause.message : String(cause);
  const data = cause.error.data;
  if (data?.kind === 'configuration_failed') return data.diagnostic;
  if (data?.kind === 'configuration_busy') return `Publication is busy: ${data.reason.replaceAll('_', ' ')}.`;
  return cause.error.message;
}

export function Settings({ client, sessionId, onClose = () => {}, theme = 'light', setTheme, onConnection }: { client: AppServerClient; sessionId?: string; onClose?: () => void; theme?: 'light' | 'dark'; setTheme?: (theme: 'light' | 'dark') => void; onConnection?: () => void }) {
  const transport = useSyncExternalStore(client.subscribe, client.getSnapshot);
  const target = sessionId ? transport.views[sessionId]?.target : undefined;
  const [scope, setScope] = useState<'effective' | SourceScope>('effective'), [section, setSection] = useState<Section>('overview');
  const [source, setSource] = useState<SourceSettings>(), [effective, setEffective] = useState<EffectiveConfiguration>();
  const [message, setMessage] = useState(''), [error, setError] = useState(''), [busy, setBusy] = useState(false);
  const writing = useRef(false), epoch = useRef(0);
  const read = useCallback(async () => {
    if (!sessionId) return { source: undefined, effective: undefined };
    const [authored, published] = await Promise.all([
      client.request({ method: 'configuration/sourcesRead', params: { session_id: sessionId } }, 'source_settings'),
      target ? client.request({ method: 'configuration/effective', params: { target } }, 'effective_configuration') : Promise.resolve(undefined),
    ]);
    return { source: authored.projection, effective: published?.projection };
  }, [client, sessionId, target]);
  const refresh = useCallback(async () => {
    const at = epoch.current;
    const next = await read();
    if (at === epoch.current) { setSource(next.source); setEffective(next.effective); }
    return next;
  }, [read]);
  useEffect(() => {
    const at = ++epoch.current;
    void read().then(next => { if (epoch.current === at) { setSource(next.source); setEffective(next.effective); } }).catch(cause => { if (epoch.current === at) setError(nativeDiagnostic(cause)); });
    return () => { ++epoch.current; };
  }, [read, transport.generation]);
  const save: SaveSource = async (mutation, expected_revision) => {
    if (writing.current) return undefined;
    writing.current = true; setBusy(true); setError(''); setMessage('');
    const at = epoch.current;
    try {
      const result = await client.request({ method: 'configuration/sourceWrite', params: { session_id: sessionId!, expected_revision, mutation } }, 'source_settings');
      if (at !== epoch.current) return undefined;
      setSource(result.projection);
      setMessage('Source saved. The loaded runtime is unchanged; Reload publishes a new generation.');
      return sourceRevision(result.projection, mutation);
    } catch (cause) {
      if (at !== epoch.current) return undefined;
      const conflict = cause instanceof RpcFailure && cause.error.data?.kind === 'source_conflict';
      setError(conflict ? 'Conflict: source changed. Your draft and original revision are preserved. Review the current source before explicitly replacing it.' : isOutcomeUncertain(cause) ? 'Save outcome uncertain. Rereading authoritative sources; no write was replayed. Your draft is preserved.' : `Save failed: ${nativeDiagnostic(cause)}`);
      try { await refresh(); } catch { /* Retain draft and uncertainty until a successful explicit read. */ }
      return undefined;
    } finally { writing.current = false; setBusy(false); }
  };
  const reload = async () => {
    if (!target || writing.current) return;
    const at = epoch.current, old = effective?.generation;
    let published = false;
    writing.current = true; setBusy(true); setError(''); setMessage('');
    try {
      await client.request({ method: 'configuration/reload', params: { target } }, 'configuration_reloaded');
      published = true;
      const current = await refresh();
      if (at === epoch.current) setMessage(`Configuration published: ${old ?? 'previous'} → ${current.effective?.generation ?? current.source?.loaded?.generation}.`);
    } catch (cause) {
      if (at !== epoch.current) return;
      if (published || isOutcomeUncertain(cause)) {
        setError('Reload outcome uncertain. Rereading the authoritative generation; the reload will not be replayed.');
        try { await refresh(); } catch { /* Current projection remains explicitly stale. */ }
      } else {
        setError(`Reload ${cause instanceof RpcFailure && cause.error.data?.kind?.includes('busy') ? 'busy' : 'failed'}: generation ${old ?? 'previous'} remains authoritative. ${nativeDiagnostic(cause)}`);
        try { await refresh(); } catch { /* Preserve the known published generation. */ }
      }
    } finally { writing.current = false; setBusy(false); }
  };
  const frame = (children: import('react').ReactNode) => <SettingsPanel rows={[{ id: 'appearance', label: 'Appearance' }, ...sections.map(([id, label]) => ({ id, label }))]} activeId={section} onSelect={id => setSection(id as Section)} onClose={onClose} actions={onConnection && <Button onClick={onConnection}>Connection</Button>}>{children}</SettingsPanel>;
  if (section === 'appearance') return frame(<section><h2>Appearance</h2><label>Theme<select aria-label="Theme" value={theme} onChange={event => setTheme?.(event.target.value as 'light' | 'dark')}><option value="light">Light</option><option value="dark">Dark</option></select></label></section>);
  if (!source) return frame(<section className={css.settings} aria-label="Settings">{error ? <p role="alert">{error}</p> : <p role="status">{sessionId ? 'Loading configuration…' : 'Open a Session to inspect its native configuration.'}</p>}<Button onClick={() => void refresh().catch(cause => setError(nativeDiagnostic(cause)))}>Read current sources</Button></section>);
  const selected = scope === 'effective' ? undefined : source[scope];
  const models = Object.keys(effective?.document.models ?? {});
  const roots = [`${source.user_resource_root}/skills`, `${source.workspace_resource_root}/skills`];
  return frame(<section className={css.settings} aria-label="Settings" aria-busy={busy}>
    <header><h2>Configuration</h2><Button disabled={busy} onClick={() => void refresh().catch(cause => setError(nativeDiagnostic(cause)))}>Read current sources</Button></header>
    <div role="tablist" aria-label="Configuration scope" className={css.tabs}>{(['effective', 'user', 'workspace'] as const).map(value => <button key={value} role="tab" aria-selected={scope === value} onClick={() => setScope(value)}>{value[0].toUpperCase() + value.slice(1)}</button>)}</div>
      <div className={css.content}>
        <div className={css.generation}><span>Runtime generation {effective?.generation ?? source.loaded?.generation ?? 'not loaded'} · {source.loaded?.pending_reload ? 'Pending reload' : 'Sources unchanged'}</span><Button variant="primary" disabled={busy || !target} onClick={() => void reload()}>Reload</Button></div>
        {message && <p role="status">{message}</p>}{error && <p className={css.error} role="alert">{error}</p>}
        <h3>{sections.find(([id]) => id === section)?.[1]}</h3>
        {selected && <><p className={css.hint}>{scope === 'user' ? 'User' : 'Workspace'} source: {selected.path}<br />Revision: {selected.revision}</p>{selected.diagnostic && <p role="alert">{selected.diagnostic}</p>}<p className={css.hint}>Edits contain authored intent only. Remove a unit to expose the lower scope; save an empty selection to select none.</p></>}
        {scope === 'effective' ? effective ? <EffectiveView value={effective} source={source} section={section} /> : <p>Attach to a loaded Session to inspect its published configuration.</p> : <fieldset disabled={busy} className={css.editor}>
          {section === 'catalog' && <CatalogEditor key={scope} document={selected?.authored ?? {}} scope={scope} revision={selected!.revision} save={save} />}
          {(section === 'general' || section === 'policies') && <RuntimeEditor key={`${scope}:${section}`} document={selected?.authored ?? {}} scope={scope} revision={selected!.revision} save={save} policyOnly={section === 'policies'} />}
          {section.startsWith('root-') && <RootEditor key={`${scope}:${section}`} document={selected?.authored ?? {}} scope={scope} revision={selected!.revision} save={save} section={section as RootSection} models={models} skillRoots={roots} />}
          {section === 'mcp' && <Integrations key={scope} source={source} scope={scope} save={save} />}
          {section === 'agents' && <AgentEditor key={scope} source={source} scope={scope} models={models} save={save} />}
          {['mcp', 'agents'].includes(section) && source.prospective_resources && <><h4>Current resource definitions</h4><ResourceInventory resources={source.prospective_resources} family={section} scope={scope} /></>}
          {['python', 'skills', 'workflows'].includes(section) && <><p>Resource definition root: {scope === 'user' ? source.user_resource_root : source.workspace_resource_root}. Root selection is edited separately.</p>{source.prospective_resources ? <><p>Current source analysis. External sources remain unprepared until admission.</p><ResourceInventory resources={source.prospective_resources} family={section} scope={scope} /></> : <p role="status">Source analysis unavailable: {source.prospective_diagnostic}</p>}</>}
          {section === 'overview' && <p>User &lt; Workspace resolution applies native semantic units. Save commits authored bytes. Reload publishes a complete runtime generation. Restart rereads current files and process bindings.</p>}
          {section === 'advanced' && <details><summary>Authored source facts (redacted)</summary><pre>{JSON.stringify(selected?.authored, null, 2)}</pre></details>}
        </fieldset>}
        <footer><h4>Source paths</h4><dl><dt>User config</dt><dd>{source.user.path}</dd><dt>User resources</dt><dd>{source.user_resource_root}</dd><dt>Workspace config</dt><dd>{source.workspace.path}</dd><dt>Workspace resources</dt><dd>{source.workspace_resource_root}</dd><dt>Runtime root</dt><dd>{source.runtime_root} · fixed for this process</dd></dl></footer>
      </div>
  </section>);
}
function EffectiveView({ value, source, section }: { value: EffectiveConfiguration; source: SourceSettings; section: Section }) {
  return <section aria-label="Effective configuration"><p>Read-only published configuration · generation {value.generation}</p>
    {section === 'overview' && <dl><dt>Root default model</dt><dd>{value.root_agent.model?.model}</dd><dt>Session explicit model</dt><dd>{value.session_model?.model ?? 'None'}</dd><dt>Effective model</dt><dd>{value.effective_model.effective.model}</dd><dt>Admitted Attempt</dt><dd>{value.admitted_attempt ? `${value.admitted_attempt.attempt} · generation ${value.admitted_attempt.generation} · ${value.admitted_attempt.model.effective.model}` : 'None'}</dd></dl>}
    {section === 'overview' && value.admitted_attempt && <details><summary>Admitted Attempt resources · frozen generation {value.admitted_attempt.generation}</summary><pre>{JSON.stringify(value.admitted_attempt.resources.main, null, 2)}</pre></details>}
    {section === 'catalog' && <><h4>Providers</h4><table><thead><tr><th>Identity</th><th>Endpoint</th><th>Origin</th></tr></thead><tbody>{Object.entries(value.document.providers ?? {}).map(([id, provider]) => <tr key={id}><td>{id}</td><td>{provider.base_url}</td><td><Origin value={value} field={`providers.${id}`} /></td></tr>)}</tbody></table><h4>Models</h4><table><thead><tr><th>Identity</th><th>Provider</th><th>Wire model</th><th>Origin</th></tr></thead><tbody>{Object.entries(value.document.models ?? {}).map(([id, model]) => <tr key={id}><td>{id}</td><td>{model.provider}</td><td>{model.id}</td><td><Origin value={value} field={`models.${id}`} /></td></tr>)}</tbody></table></>}
    {section.startsWith('root-') && <><RootFacts value={value} section={section} />{section === 'root-skills' && <p>Prompt visibility, not filesystem access.<br />User Skill root: {source.user_resource_root}/skills<br />Workspace Skill root: {source.workspace_resource_root}/skills</p>}</>}
    {section === 'general' && <dl><dt>Approval mode</dt><dd>{value.approval_mode}</dd><dt>Context</dt><dd>{JSON.stringify(value.context)}</dd><dt>Model timeout</dt><dd>{JSON.stringify(value.model_timeout)}</dd><dt>Tool deadline</dt><dd>{JSON.stringify(value.tool_deadline)}</dd><dt>Child capacity</dt><dd>{value.child_capacity.maxConcurrent}</dd></dl>}
    {section === 'policies' && <table><thead><tr><th>Tool</th><th>Execution</th><th>Concurrency</th><th>Approval</th><th>Policy origin</th></tr></thead><tbody>{value.available_tools.map(tool => <tr key={tool.id}><td>{tool.name}</td><td>{JSON.stringify(tool.execution_policy)}</td><td>{JSON.stringify(tool.concurrency_policy)}</td><td>{JSON.stringify(tool.approval_policy)}</td><td><Origin value={value} field={tool.origin === 'builtin' ? `native_tools.${tool.name}` : 'mcp' in tool.origin ? `mcp_tool_policies.${tool.origin.mcp.server_id}` : 'python-native-policy'} /></td></tr>)}</tbody></table>}
    {['mcp', 'python', 'skills', 'agents', 'workflows'].includes(section) && <ResourceInventory resources={value.resources} family={section} />}
    {section === 'advanced' && <><h4>Diagnostics</h4>{value.resources.resource_diagnostics.map((diagnostic, index) => <p key={index}>{diagnostic.identity}: {diagnostic.reason} {diagnostic.file}</p>)}<details><summary>Native provenance and source revisions</summary><pre>{JSON.stringify({ provenance: value.provenance, source_revisions: value.source_revisions }, null, 2)}</pre></details></>}
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
  return <dl>{facts.map(([name, fact, field]) => <div key={field}><dt>{name}</dt><dd>{typeof fact === 'string' ? fact : JSON.stringify(fact)} · <Origin value={value} field={field} /></dd></div>)}</dl>;
}
function Origin({ value, field }: { value: EffectiveConfiguration; field: string }) { const origin = value.provenance[field]; return <span>{origin ? origin.kind === 'builtin' ? 'Product default' : origin.kind === 'process' ? 'Process binding' : `${origin.kind}: ${origin.document}` : 'Native domain default'}</span>; }
function ResourceInventory({ resources, family, scope }: { resources: CapabilityInspection; family: string; scope?: SourceScope }) {
  const nativeFamily = ({ agents: 'agent', workflows: 'workflow', python: 'managed_python', skills: 'skill', mcp: 'mcp' } as const)[family as 'agents' | 'workflows' | 'python' | 'skills' | 'mcp'];
  const entries = resources.definitions.filter(entry => entry.family === nativeFamily && (!scope || entry.location.scope === scope || (scope === 'user' && entry.location.shadowed != null)));
  return <ul>{entries.map(entry => {
    if (scope === 'user' && entry.location.scope === 'workspace') return <li key={entry.name}>{entry.name} · user · {entry.location.shadowed} · Shadowed by Workspace · {entry.location.path}</li>;
    const sourceId = entry.family === 'managed_python' ? `python:${entry.name}` : entry.name;
    const selected = entry.family === 'skill' ? resources.main?.skills.some(skill => skill.name === entry.name)
      : entry.family === 'agent' ? resources.main?.agents.includes(entry.name)
      : entry.family === 'workflow' ? resources.main?.workflows.includes(entry.name)
      : resources.main?.tool_selection.some(selection => selection.origin !== 'builtin' && selection.source_id === sourceId);
    const readiness = entry.family === 'mcp' || entry.family === 'managed_python' ? resources.sources[sourceId]?.status ?? 'unprepared' : entry.family === 'workflow' ? resources.workflows[entry.name]?.status : undefined;
    return <li key={entry.name}>{entry.name} · {entry.location.scope} · {entry.location.path} · {entry.valid ? 'Valid definition' : 'Invalid definition'} · {selected ? entry.family === 'skill' ? 'Root visible' : 'Root selected' : 'Defined only'}{readiness && ` · ${readiness}`}{entry.location.shadowed && <p>Shadows {entry.location.shadowed}</p>}</li>;
  })}</ul>;
}
