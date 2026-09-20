import { useState } from 'react';
import type { CapabilityInspection, ResourceFamily, SourceScope } from '../../../../protocol/app-server/v14';
import { Badge, Facts, SettingsCard } from '../../presentation/settings/SettingsContent';
import css from '../../presentation/settings/SettingsContent.module.css';
import { NativeFacts } from '../components/NativeFacts';

const families: Record<string, ResourceFamily> = { agents: 'agent', workflows: 'workflow', python: 'managed_python', skills: 'skill', mcp: 'mcp' };
/** Inventory and selection are independent native facts, even for invalid winners. */
export function ResourceInventory({ resources, family, scope }: { resources: CapabilityInspection; family: string; scope?: SourceScope }) {
  const [query, setQuery] = useState('');
  const entries = resources.definitions.filter(entry => entry.family === families[family] && (!scope || entry.location.scope === scope || (scope === 'user' && entry.location.shadowed != null)) && entry.name.toLocaleLowerCase().includes(query.toLocaleLowerCase()));
  return <section aria-label={`${family} inventory`}>
    <label>Filter definitions<input value={query} onChange={e => setQuery(e.target.value)} placeholder="Find by identity" /></label>
    {family === 'skills' && <p>Skill selection controls prompt visibility, not a filesystem ACL. Package paths are native inspection facts.</p>}
    {family === 'workflows' && <p>Native Workflow definitions and admission status. This protocol has no Workflow source-write operation; author programs in the native resource source.</p>}
    <div className={css.rows}>{entries.map(entry => {
      if (scope === 'user' && entry.location.scope === 'workspace') return <SettingsCard key={entry.name} title={entry.name} meta={<Badge>Shadowed</Badge>}><p>Shadowed by Workspace · {entry.location.shadowed}</p><p className={css.hint}>Winner: {entry.location.path}. The losing definition is not parsed or prepared.</p></SettingsCard>;
      const sourceId = entry.family === 'managed_python' ? `python:${entry.name}` : entry.name;
      const selection = resources.main?.tool_selection.filter(selected => selected.origin !== 'builtin' && selected.source_id === sourceId);
      const selected = entry.family === 'skill' ? resources.main?.skills.some(skill => skill.name === entry.name)
        : entry.family === 'agent' ? resources.main?.agents.includes(entry.name)
        : entry.family === 'workflow' ? resources.main?.workflows.includes(entry.name) : !!selection?.length;
      const source = resources.sources[sourceId], workflow = resources.workflows[entry.name], agent = resources.agents[entry.name];
      const readiness = entry.family === 'mcp' || entry.family === 'managed_python' ? source?.status ?? 'unprepared' : entry.family === 'workflow' ? workflow?.status ?? 'Not inspected' : undefined;
      return <SettingsCard key={entry.name} title={entry.name} meta={<><Badge tone={entry.valid ? 'success' : 'error'}>{entry.valid ? 'Valid definition' : 'Invalid definition'}</Badge><Badge>{selected ? entry.family === 'skill' ? 'Root visible' : 'Root selected' : 'Defined only'}</Badge></>}>
        <Facts rows={[[ 'Scope', entry.location.scope ], ['Source', entry.location.path], ...(entry.location.shadowed ? [['Shadows', entry.location.shadowed] as const] : []), ...(readiness ? [['Preparation', readiness] as const] : [])]} />
        {selection?.length ? <details><summary>Root source selection</summary><NativeFacts value={selection} /></details> : null}
        {source && <details><summary>Source Tools & preparation</summary><NativeFacts value={source} /></details>}
        {agent && <details><summary>Independent Agent profile</summary><NativeFacts value={agent} /></details>}
        {workflow?.status === 'disabled' && <details open><summary>Workflow admission diagnostics</summary><NativeFacts value={workflow.diagnostics} /></details>}
        {resources.resource_diagnostics.filter(item => item.identity === entry.name || item.file === entry.location.path).map((item, index) => <p className={css.error} key={index}>{item.reason}</p>)}
      </SettingsCard>;
    })}</div>
    {!entries.length && <p className={css.hint}>No matching definitions in this native projection.</p>}
    {family === 'skills' && !!resources.skill_diagnostics.length && <details open><summary>Skill package diagnostics</summary><NativeFacts value={resources.skill_diagnostics} /></details>}
  </section>;
}
