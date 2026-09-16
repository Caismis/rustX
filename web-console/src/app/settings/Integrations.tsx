/* Copyright (c) 2026 DeepSeek. MIT. Adapted disclosure cards and inventory facts; see PROVENANCE.md. */
import { useState } from 'react';
import type { SourceSettings, SourceMutation, McpDraft, IntegrationScope, IntegrationControl, RuntimeClientSnapshot, InvocationPolicyDocument } from '../../../../protocol/app-server/v5';
import { Button } from '../../presentation/primitives/Button';
import css from './Settings.module.css';

type Draft = { scope: IntegrationScope; id: string; value: McpDraft; existing: boolean; revision: string; attempted: boolean };
const empty = (): McpDraft => ({ enabled: false, transport: 'stdio', command: '', args: [], cwd: null, url: null, retained_env: [], retained_headers: [], sensitive_env: {}, sensitive_headers: {} });
export function Integrations({ source, snapshot, busy, save }: {
  source: SourceSettings; snapshot?: RuntimeClientSnapshot; busy: boolean;
  save: (scope: IntegrationScope, mutation: SourceMutation, expectedRevision?: string) => Promise<boolean>;
}) {
  const [scope, setScope] = useState<IntegrationScope>('user');
  const [draft, setDraft] = useState<Draft>();
  const [query, setQuery] = useState('');
  const state = source.integrations;
  const label = scope === 'user' ? 'User' : 'Workspace';
  const blocked = busy || (scope === 'workspace' && !source.workspace.active);
  const change = (patch: Partial<McpDraft>) => setDraft(current => current && ({ ...current, value: { ...current.value, ...patch } }));
  const commit = async (remove = false) => {
    if (!draft) return;
    if (await save(draft.scope, { kind: 'mcp', scope: draft.scope, id: draft.id, authored: remove ? null : draft.value }, draft.attempted ? source[draft.scope].revision : draft.revision)) setDraft(undefined);
    else setDraft(current => current && ({ ...current, attempted: true }));
  };
  const control = (control: IntegrationControl) => save(scope, { kind: 'integration', scope, control });
  const selected = state[scope];
  const inventory = state.inventory;
  return <section aria-label="Integrations">
    <h3>MCP configuration and integration inventory</h3>
    <p>Persistent changes apply on fresh / cold resolution. Source saves do not connect servers, replace loaded resources, or change admitted work.</p>
    <label>Integration scope<select disabled={busy || !!draft} value={scope} onChange={e => setScope(e.target.value as IntegrationScope)}><option value="user">User</option><option value="workspace">Workspace</option></select></label>
    <p className={css.hint}>Scope: {label} · Target: {source[scope].document}<br />Revision: {source[scope].revision}</p>
    <p>Workspace trusted: {source.workspace.active ? 'Yes' : 'No — editing unavailable. Host cwd authorization does not imply rustX trust.'}</p>
    <label>Find MCP server<input type="search" value={query} onChange={e => setQuery(e.target.value)} /></label>
    {!state.mcp_valid && <p role="alert">Native MCP validation failed. This does not establish runtime admissibility.</p>}
    {state.mcp.filter(entry => entry.id.toLowerCase().includes(query.toLowerCase())).map(entry => <article key={entry.id} className={css.card}>
      <h4>{entry.id}</h4>
      <p>Definition winner: {entry.winning?.kind === 'project' ? 'Workspace' : entry.winning?.kind === 'user' ? 'User' : 'None'}</p>
      <p>User definition: {entry.user ? 'present' : 'absent'} · Workspace definition: {source.workspace.active ? entry.workspace ? 'present' : 'absent' : 'not inspected (untrusted)'}</p>
      {entry.user && entry.winning?.kind === 'project' && <p>Shadowed User definition — editable independently</p>}
      <p>Prospective activation: {entry.activation ?? 'Not active'}</p>
      <details><summary>Source and runtime facts</summary>
        <p className={css.hint}>Origin: {entry.winning && 'document' in entry.winning ? entry.winning.document : 'Unavailable'}</p>
        <p>Loaded source: {snapshot?.capabilities.sources?.filter(row => row.source.type === 'mcp' && row.source.server_id === entry.id).map(row => row.state.type).join(', ') || 'No runtime observation'}</p>
        <p>Discovery and enablement do not prove connection or Tool exposure.</p>
      </details>
      <p>Authorized MCP definition: {entry.winning ? 'present' : 'absent'}</p>
      <p>User Tool policy: {entry.policy_state === 'absent' ? 'absent — native default' : entry.policy_state === 'dangling' ? 'authored — dangling' : 'authored'}</p>
      {entry.policy_state === 'dangling' && <p role="alert">Current MCP domain: invalid — User policy has no authorized MCP definition.</p>}
      <PolicyEditor key={entry.id} id={entry.id} policy={state.mcp_tool_policies[entry.id]} revision={source.user.revision} busy={busy} save={(authored, revision) => save('user', { kind: 'mcp_policy', id: entry.id, authored }, revision)} />
      <Button disabled={blocked || !!draft} onClick={() => setDraft({ scope, revision: source[scope].revision, attempted: false, id: entry.id, existing: !!entry[scope], value: structuredClone(entry[scope] ?? empty()) })}>{entry[scope] ? 'Edit' : 'Define'} {label} · {entry.id}</Button>
    </article>)}
    {!state.mcp.length && <p>No MCP definitions in authorized sources.</p>}
    <Button disabled={blocked || !!draft} onClick={() => setDraft({ scope, revision: source[scope].revision, attempted: false, id: '', value: empty(), existing: false })}>Add {label} MCP server</Button>
    {draft && <fieldset className={css.card} disabled={busy || (draft.scope === 'workspace' && !source.workspace.active)}><legend>MCP structured draft · {draft.scope}</legend>
      <p>Target: {draft.id || 'New identity'} · Draft revision: {draft.revision} · Next explicit save revision: {draft.attempted ? source[draft.scope].revision : draft.revision}</p>
      <label>Server identity<input required disabled={draft.existing} value={draft.id} onChange={e => setDraft({ ...draft, id: e.target.value })} /></label>
      <label>Activation<select value={draft.value.enabled == null ? 'unconfigured' : draft.value.enabled ? 'enabled' : 'disabled'} onChange={e => change({ enabled: e.target.value === 'unconfigured' ? null : e.target.value === 'enabled' })}><option value="unconfigured">Unconfigured · inert</option><option value="disabled">Disabled · inert</option><option value="enabled">Enabled · eligible for native preparation</option></select></label>
      <label>Transport<select value={draft.value.transport ?? ''} onChange={e => change({ transport: e.target.value === '' ? null : e.target.value as 'stdio' | 'http' })}><option value="">Infer from native fields</option><option value="stdio">stdio</option><option value="http">HTTP</option></select></label>
      <p className={css.hint}>Rust validates field combinations. Clear fields from the previous transport when switching.</p>
      <label>Command<input value={draft.value.command ?? ''} onChange={e => change({ command: e.target.value || null })} /></label>
      <StringList label="Arguments" values={draft.value.args} change={args => change({ args })} />
      <label>Working directory<input value={draft.value.cwd ?? ''} onChange={e => change({ cwd: e.target.value || null })} /></label>
      <label>URL<input value={draft.value.url ?? ''} onChange={e => change({ url: e.target.value || null })} /></label>
      <p>Existing ordinary environment/header values are private. Retain or remove them; use User-owned references for credentials.</p>
      {(['retained_env', 'retained_headers'] as const).map(field => <div key={field}>{draft.value[field].map(name => <label key={name}><input type="checkbox" checked onChange={() => change({ [field]: draft.value[field].filter(item => item !== name) })} />Retain {field === 'retained_env' ? 'environment' : 'header'}: {name} (value hidden)</label>)}</div>)}
      {draft.scope === 'user' && <><References label="Environment references" values={draft.value.sensitive_env} change={sensitive_env => change({ sensitive_env })} /><References label="Header references" values={draft.value.sensitive_headers} change={sensitive_headers => change({ sensitive_headers })} /></>}
      <p className={css.hint}>Reference metadata only. Credential presence is checked by the native owner during preparation; this form never reads secrets.</p>
      <div className={css.actions}><Button onClick={() => setDraft(undefined)}>Discard MCP draft</Button><Button disabled={!draft.id} variant="primary" onClick={() => void commit()}>Save MCP</Button>{draft.existing && <Button onClick={() => void commit(true)}>Delete authored MCP entry</Button>}</div>
    </fieldset>}
    <h3>Existing resources and activation</h3>
    {!inventory && <p role="status">Static resource inventory unavailable: full native configuration/resource validation did not succeed. MCP authoring remains independent.</p>}
    <fieldset disabled={blocked} className={css.card}><legend>Named Agents · root selection</legend>
      <p>Definition provenance is separate from root agent.agents selection.</p>
      {state.agents.map(agent => <div key={agent.name}><label><input type="checkbox" checked={selected.agents?.includes(agent.name) ?? false} onChange={e => void control({ kind: 'agents', selected: e.target.checked ? [...(selected.agents ?? []), agent.name] : (selected.agents ?? []).filter(name => name !== agent.name) })} />{agent.name}</label><p className={css.hint}>Definition: {agent.selected}{agent.shadowed ? ` · Shadowed: ${agent.shadowed}` : ''}</p></div>)}
      <p>Authored root selection: {selected.agents?.join(', ') ?? 'Inherited'}</p><Button onClick={() => void control({ kind: 'agents', selected: null })}>Inherit Agent selection</Button>
    </fieldset>
    <fieldset disabled={blocked} className={css.card}><legend>Workflows · root selection</legend><p>Workspace program definitions are read-only. Selection authors agent.workflows.</p>
      {Object.entries(inventory?.workflows ?? {}).map(([name, fact]) => <label key={name}><input type="checkbox" checked={selected.workflows?.includes(name) ?? false} onChange={e => void control({ kind: 'workflows', selected: e.target.checked ? [...(selected.workflows ?? []), name] : (selected.workflows ?? []).filter(id => id !== name) })} />{name} · {fact.status}</label>)}
      <p>Authored root selection: {selected.workflows?.join(', ') ?? 'Inherited'}</p><Button onClick={() => void control({ kind: 'workflows', selected: null })}>Inherit Workflow selection</Button>
    </fieldset>
    <fieldset disabled={blocked} className={css.card}><legend>Skills · source policy and root visibility</legend>
      <p>Source policy: {selected.skill_sources?.join(', ') ?? 'Inherited'}. Resource existence, visibility, Session paths, and admitted catalogs are separate.</p>
      {(['global', 'workspace'] as const).map(name => <label key={name}><input type="checkbox" checked={selected.skill_sources?.includes(name) ?? false} onChange={e => void control({ kind: 'skill_sources', selected: e.target.checked ? [...(selected.skill_sources ?? []), name] : (selected.skill_sources ?? []).filter(id => id !== name) })} />Automatic {name} Skills</label>)}
      <Button onClick={() => void control({ kind: 'skill_sources', selected: null })}>Inherit Skill source policy</Button>
      {inventory?.skills.map((skill, index) => <div key={`${skill.name}:${index}`}><p>{skill.name} · {skill.source}<br /><span className={css.hint}>{skill.location}</span></p>{skill.shadowed.map(shadow => <p key={shadow.location} className={css.hint}>Shadowed {shadow.source}: {shadow.location}</p>)}<label><input type="checkbox" checked={selected.disabled_skills?.includes(skill.name) ?? false} onChange={e => void control({ kind: 'skill_visibility', disabled: e.target.checked ? [...(selected.disabled_skills ?? []), skill.name] : (selected.disabled_skills ?? []).filter(id => id !== skill.name) })} />Hide {skill.name} from root</label></div>)}
      <Button onClick={() => void control({ kind: 'skill_visibility', disabled: null })}>Inherit Skill visibility</Button>
    </fieldset>
    <fieldset disabled={blocked} className={css.card}><legend>Native Agent Extensions</legend><p>A toggle preserves siblings authored at this scope. The collection replaces the lower scope as a whole. Todo/Goal runtime state is separate.</p>
      {(['todo', 'goal', 'agent_status'] as const).map(identity => <label key={identity}><input type="checkbox" checked={selected.extensions?.[identity]?.enabled ?? false} onChange={e => void control({ kind: 'extension', identity, enabled: e.target.checked })} />{identity}</label>)}
      <Button onClick={() => void control({ kind: 'reset_extensions' })}>Inherit Extensions</Button>
    </fieldset>
    <details className={css.card}><summary>Selection provenance · prospective effective configuration</summary>{Object.entries(state.provenance).map(([key, value]) => <p key={key} className={css.hint}>{key}: {value.kind}{'document' in value ? ` · ${value.document}` : ''}</p>)}<p>{JSON.stringify(state.prospective)}</p></details>
    <article className={css.card}><h3>Effective capabilities · read only</h3>
      {snapshot ? <><p>Loaded resource generation: {snapshot.resources?.revision} · Capability revision: {snapshot.capabilities.revision}</p><p>Admitted attempt: {snapshot.attempt?.execution_settings ? `Frozen resource generation ${snapshot.attempt.execution_settings.resource_revision}` : 'No frozen generation observation'}</p>
        <h4>Tools / Python availability</h4>{snapshot.capabilities.sources?.map((row, index) => <p key={index}>{JSON.stringify(row.source)} · {row.state.type}</p>)}
        <h4>Currently exposed Tools</h4>{snapshot.capabilities.tools?.map(tool => <p key={tool.id}>{tool.name} · {JSON.stringify(tool.origin)}</p>)}
        <h4>Admitted Skills</h4>{snapshot.capabilities.skills?.map(skill => <p key={skill.name}>{skill.name}</p>)}
        <details><summary>Native resource inspection</summary><p>Root Agents: {snapshot.resources?.inspection.main?.agents.join(', ') || 'None'}</p><p>Root Workflows: {snapshot.resources?.inspection.main?.workflows.join(', ') || 'None'}</p>{snapshot.resources?.inspection.main?.extensions.map(item => <p key={item.identity}>{item.identity}: {item.active ? 'active' : 'inactive'}</p>)}</details>
      </> : <p>No loaded runtime observation. Prospective sources do not establish readiness.</p>}
    </article>
  </section>;
}
function StringList({ label, values, change }: { label: string; values: string[]; change: (values: string[]) => void }) {
  return <div><p>{label}</p>{values.map((value, index) => <label key={index}>{label} {index + 1}<input aria-label={`${label} ${index + 1}`} value={value} onChange={e => change(values.map((old, i) => i === index ? e.target.value : old))} /><Button onClick={() => change(values.filter((_, i) => i !== index))}>Remove {label} {index + 1}</Button></label>)}<Button onClick={() => change([...values, ''])}>Add {label}</Button></div>;
}
function References({ label, values, change }: { label: string; values: Record<string, string>; change: (values: Record<string, string>) => void }) {
  const [name, setName] = useState(''), [reference, setReference] = useState('');
  return <div><h4>{label}</h4>{Object.entries(values).map(([key, value]) => <p key={key}>{key} → {value} <Button onClick={() => { const next = { ...values }; delete next[key]; change(next); }}>Remove {key}</Button></p>)}
    <label>{label} name<input value={name} onChange={e => setName(e.target.value)} /></label><label>{label} reference<input placeholder="$ENV_VAR" value={reference} onChange={e => setReference(e.target.value)} /></label>
    <Button disabled={!name || !/^\$[A-Za-z_][A-Za-z0-9_]*$/.test(reference)} onClick={() => { change({ ...values, [name]: reference }); setName(''); setReference(''); }}>Add {label}</Button>
  </div>;
}

type PolicyDraft = { value: InvocationPolicyDocument; revision: string; attempted: boolean };
function PolicyEditor({ id, policy, revision, busy, save }: { id: string; policy?: InvocationPolicyDocument; revision: string; busy: boolean; save: (policy: InvocationPolicyDocument | null, revision: string) => Promise<boolean> }) {
  const [draft, setDraft] = useState<PolicyDraft>();
  const value = draft?.value ?? policy ?? { approval: 'never', execution: 'foreground_only', concurrency: 'sequential' };
  const begin = () => setDraft(current => current ?? { value: { ...value }, revision, attempted: false });
  const change = (patch: Partial<InvocationPolicyDocument>) => setDraft(current => ({ value: { ...value, ...patch }, revision: current?.revision ?? revision, attempted: current?.attempted ?? false }));
  const commit = async (remove = false) => {
    const pending = draft ?? { value: { ...value }, revision, attempted: false };
    setDraft(pending);
    if (await save(remove ? null : pending.value, pending.attempted ? revision : pending.revision)) setDraft(undefined);
    else setDraft(current => current && ({ ...current, attempted: true }));
  };
  return <details onToggle={event => { if (event.currentTarget.open) begin(); }}><summary>User-owned Tool policy · {id}</summary><fieldset disabled={busy}><p>Scope: User · Target: mcp_tool_policies.{id}. Workspace/Session cannot author this ceiling.</p>
    <p>User revision: {revision}{draft && <> · Draft revision: {draft.revision} · Next explicit save revision: {draft.attempted ? revision : draft.revision}</>}</p>
    <label>Approval · {id}<select value={value.approval} onChange={e => change({ approval: e.target.value as 'never' | 'always' })}><option value="never">Never</option><option value="always">Always</option></select></label>
    <label>Execution · {id}<select value={value.execution} onChange={e => change({ execution: e.target.value as InvocationPolicyDocument['execution'] })}>{['foreground_only', 'background_only', 'model_selectable'].map(value => <option key={value}>{value}</option>)}</select></label>
    <label>Concurrency · {id}<select value={value.concurrency} onChange={e => change({ concurrency: e.target.value as 'sequential' | 'parallel' })}><option value="sequential">sequential</option><option value="parallel">parallel</option></select></label>
    <Button onClick={() => setDraft(undefined)}>Discard policy draft</Button><Button onClick={() => void commit()}>{draft?.attempted ? 'Retry User Tool policy' : 'Save User Tool policy'}</Button><Button onClick={() => void commit(true)}>Reset User Tool policy</Button>
  </fieldset></details>;
}
