import { useState } from 'react';
import type { AgentProfileDocument, ModelLayer, SourceScope, SourceSettings, AgentStatusExtensionDocument } from '../../../../protocol/app-server/v14';
import { Button } from '../../presentation/primitives/Button';
import { Badge, SettingsCard } from '../../presentation/settings/SettingsContent';
import css from '../../presentation/settings/SettingsContent.module.css';
import { RequestPolicy } from './RequestPolicy';
import { CheckboxList, Toggle, OptionalBoolean, Names, nativeTools, Selection, TextField, UnitForm, type SaveSource } from './controls';

function ModelRequestFields<T extends Pick<ModelLayer, 'reasoning_profile' | 'max_output_tokens' | 'request_params'>>({ value, change, prefix = '' }: { value: T; change: (next: T) => void; prefix?: string }) {
  return <><label>{prefix}Reasoning profile<select value={value.reasoning_profile?.mode ?? ''} onChange={e => change({ ...value, reasoning_profile: e.target.value === 'profile' ? { mode: 'profile', name: '' } : e.target.value === 'catalog_default' ? { mode: 'catalog_default' } : null })}><option value="">Domain default</option><option value="catalog_default">Catalog default</option><option value="profile">Named profile</option></select></label>
    {value.reasoning_profile?.mode === 'profile' && <TextField label={`${prefix}Profile identity`} required value={value.reasoning_profile.name} change={name => change({ ...value, reasoning_profile: { mode: 'profile', name } })} />}
    <label>{prefix}Output limit<input type="number" min="1" value={value.max_output_tokens?.mode === 'limit' ? value.max_output_tokens.tokens : ''} onChange={e => change({ ...value, max_output_tokens: e.target.value ? { mode: 'limit', tokens: Number(e.target.value) } : { mode: 'catalog_default' } })} /></label>
    <RequestPolicy value={value.request_params ?? {}} change={request_params => change({ ...value, request_params })} />
  </>;
}
export function ModelSelectionFields({ value, change, models }: { value: ModelLayer; change: (next: ModelLayer) => void; models: string[] }) {
  const summary = value.summary_model;
  return <><label>Model<select required value={value.model ?? ''} onChange={e => change({ ...value, model: e.target.value })}><option value="">Select model</option>{[...new Set([...models, ...(value.model ? [value.model] : [])])].map(id => <option key={id}>{id}</option>)}</select></label>
    <ModelRequestFields value={value} change={change} />
    <label>Summary model<select value={summary?.mode === 'explicit' ? summary.model : ''} onChange={e => change({ ...value, summary_model: e.target.value ? { ...(summary?.mode === 'explicit' ? summary : {}), mode: 'explicit', model: e.target.value } : { mode: 'session' } })}><option value="">Follow selected model</option>{[...new Set([...models, ...(summary?.mode === 'explicit' ? [summary.model] : [])])].map(id => <option key={id}>{id}</option>)}</select></label>
    {summary?.mode === 'explicit' && <fieldset><legend>Explicit Summary Model settings</legend><ModelRequestFields value={summary} prefix="Summary " change={summary_model => change({ ...value, summary_model })} /></fieldset>}
  </>;
}
export function SourceSelections({ value, change }: { value: Record<string, 'all' | string[]>; change: (value: Record<string, 'all' | string[]>) => void }) {
  const [id, setId] = useState(''), [family, setFamily] = useState('mcp');
  return <><p>MCP and Managed Python definitions do not activate Tools. Select each source explicitly.</p>
    {Object.entries(value).map(([source, selection]) => <div key={source}><Selection label={source} value={selection} change={next => change({ ...value, [source]: next })} /><Button onClick={() => { const next = { ...value }; delete next[source]; change(next); }}>Remove source selection {source}</Button></div>)}
    <label>Source family<select value={family} onChange={e => setFamily(e.target.value)}><option value="mcp">MCP</option><option value="python">Managed Python</option></select></label><TextField label="Source identity" value={id} change={setId} /><Button disabled={!id} onClick={() => { change({ ...value, [family === 'python' ? `python:${id}` : id]: [] }); setId(''); }}>Add source selection</Button>
  </>;
}
export function StatusPluginFields({ value, change }: { value: AgentStatusExtensionDocument; change: (value: AgentStatusExtensionDocument) => void }) {
  return <><Toggle label="Enable Agent Status Plugin" checked={value.enabled ?? false} change={enabled => change({ ...value, enabled })} />
    <OptionalBoolean label="Time contributor" value={value.time?.enabled} change={enabled => change({ ...value, time: { ...value.time, enabled } })} /><TextField label="Time zone (IANA)" value={value.time?.timezone ?? ''} change={timezone => change({ ...value, time: { ...value.time, timezone: timezone || null } })} />
    <OptionalBoolean label="Background contributor" value={value.background?.enabled} change={enabled => change({ ...value, background: { ...value.background, enabled } })} /></>;
}
export function AgentEditor({ source, scope, models, save }: { source: SourceSettings; scope: SourceScope; models: string[]; save: SaveSource }) {
  const [selected, select] = useState(''), [name, setName] = useState('');
  const agents = source.agents.filter(agent => agent.scope === scope);
  const current = agents.find(agent => agent.name === selected);
  return <section aria-label="Named Agents"><h3>Named Agents</h3><p>Each resource is an independent complete profile. Workspace shadows the whole same-name User resource, including invalid definitions. Root delegates only to its named-Agent allowlist.</p>
    <div className={css.rows}>{agents.map(agent => <SettingsCard key={agent.name} title={agent.name} meta={<Badge>{scope}</Badge>} actions={<Button onClick={() => select(agent.name)}>Edit Agent {agent.name}</Button>}><p className={css.hint}>{agent.source.path}</p>{agent.source.diagnostic && <p role="alert">{agent.source.diagnostic}</p>}</SettingsCard>)}</div>
    <TextField label="New Agent identity" value={name} change={setName} /><Button disabled={!name || agents.some(agent => agent.name === name)} onClick={() => { select(name); setName(''); }}>Add Agent</Button>
    {selected && <UnitForm<AgentProfileDocument> key={selected} title={`Agent ${selected}`} initial={current?.source.authored ?? {}} revision={current?.source.revision ?? source.absent_resource_revision} mutation={authored => ({ kind: 'agent', scope, name: selected, authored })} save={save}>{(value, change) => <>
      <TextField label="Description" value={value.description} change={description => change({ ...value, description })} />
      <label>Instructions<textarea value={value.instructions ?? ''} onChange={e => change({ ...value, instructions: e.target.value })} /></label>
      <label><input type="checkbox" checked={!!value.model} onChange={e => change({ ...value, model: e.target.checked ? {} : null })} />Explicit child model</label>
      {value.model ? <ModelSelectionFields value={value.model} change={model => change({ ...value, model })} models={models} /> : <p>Inherit the invoking Attempt's already-frozen effective model.</p>}
      <CheckboxList label="Native Tools" values={nativeTools} selected={value.tools?.builtin ?? []} change={builtin => change({ ...value, tools: { ...value.tools, builtin } })} />
      <SourceSelections value={value.tools?.sources ?? {}} change={sources => change({ ...value, tools: { ...value.tools, sources } })} />
      <Names label="Delegated Agents" value={value.agents ?? []} change={agents => change({ ...value, agents })} />
      <Names label="Delegated Workflows" value={value.workflows ?? []} change={workflows => change({ ...value, workflows })} />
      <p>Native scope validation decides which capabilities this profile may compose.</p>
      <Selection label="Skill prompt visibility" value={value.skills ?? []} change={skills => change({ ...value, skills })} />
      <fieldset><legend>Plugins · default off</legend>{(['todo', 'goal'] as const).map(id => <label key={id}><input type="checkbox" checked={value.plugins?.[id]?.enabled ?? false} onChange={e => change({ ...value, plugins: { ...value.plugins, [id]: { enabled: e.target.checked } } })} />{id}</label>)}<StatusPluginFields value={value.plugins?.agent_status ?? {}} change={agent_status => change({ ...value, plugins: { ...value.plugins, agent_status } })} /></fieldset>
      <label>Child timeout (ms)<input type="number" min="1" value={value.timeout_ms ?? ''} onChange={e => change({ ...value, timeout_ms: e.target.value || null })} /></label>
      <label><input type="checkbox" checked={value.worktree?.enabled ?? false} onChange={e => change({ ...value, worktree: { ...value.worktree, enabled: e.target.checked } })} />Use child worktree</label>
      <label><input type="checkbox" checked={value.worktree?.require_clean_parent ?? true} onChange={e => change({ ...value, worktree: { ...value.worktree, require_clean_parent: e.target.checked } })} />Require clean parent</label>
      <label><input type="checkbox" checked={value.agents_md?.inherit ?? true} onChange={e => change({ ...value, agents_md: { ...value.agents_md, inherit: e.target.checked } })} />Include project guidance</label>
      <Names label="Project guidance files" value={value.agents_md?.files ?? []} change={files => change({ ...value, agents_md: { ...value.agents_md, files } })} />
    </>}</UnitForm>}
  </section>;
}
