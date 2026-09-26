import { useTranslation } from '../../../locale/react';
import { useState } from 'react';
import type {
  AgentSkillSelection, ApprovalMode, NativePolicyOverrideDocument, NativeTool,
  RuntimeLayer, SourceScope, SourceSettings, SourceToolSelection,
} from '../../../../../protocol/app-server/v23';
import { Button } from '../../../presentation/primitives/Button';
import { UnitForm } from '../forms/bridge';
import { CheckboxList, Names, Selection, TextField, nativeTools, policyTools } from '../forms/controls';
import { Advanced, Choice } from '../primitives/aria';
import { reachableIdentities, sourceView } from '../projection';
import css from '../../../presentation/settings/SettingsContent.module.css';

/** The Tools & Permissions product page.
 *
 * It answers exactly one user question — *what may the root Agent use, and
 * under what policy?* Approval behavior, built-in Tool availability, access to
 * MCP and Managed Python Tool sources, Skill visibility and delegation to
 * named Agents and Workflows are all that question, so they are all here.
 *
 * Every control keeps its exact native identity and its exact native
 * vocabulary. `all`, an explicit list of identities, an explicit empty list and
 * an absent unit are four different authored values and are never collapsed
 * into one browser state.
 *
 * There is no Session permission overlay here and no shortcut that writes User
 * or Workspace policy from a Session surface. Execution-time approval and
 * ask-user interactions remain a separate runtime concern and are untouched. */
export function ToolsPage({ source, document, scope, revision }: {
  source: SourceSettings; document: RuntimeLayer; scope: SourceScope; revision: string;
}) {
  const tx = useTranslation();
  const resolved = source.resolved;
  const agent = document.agent;
  const [source_, setSource] = useState('');
  const [family, setFamily] = useState('mcp');
  const [added, setAdded] = useState<string[]>([]);
  const [mcpPolicy, setMcpPolicy] = useState('');
  const [selectedMcpPolicy, selectMcpPolicy] = useState('');
  const sources = [...new Set([...reachableIdentities(scope, agent?.tools?.sources, resolved?.agent?.tools?.sources), ...added])];
  const skillRoots = [
    source.user_resource_root ? `${source.user_resource_root}/skills` : '',
    source.workspace_resource_root ? `${source.workspace_resource_root}/skills` : '',
  ];
  return <section aria-label={tx('settings:tools-page.tools-permissions')}>
    <h3>{tx('settings:tools-page.tools-amp-permissions')}</h3>
    <p>{tx('settings:tools-page.what-the-root-agent-may-use-and-under-what-policy-defining-a-res')}</p>

    <h4>{tx('settings:tools-page.approval')}</h4>
    <UnitForm<ApprovalMode> title={tx('settings:tools-page.approval-mode')} authored={document.approval_mode ?? undefined} blank="policy" revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'approval', authored } })}>
      {(value, change) => <Choice label={tx('settings:tools-page.approval-mode')} value={value}
        options={[['policy', tx('settings:copy.policy')], ['full_access', tx('settings:copy.full-access')]]} onChange={change} />}
    </UnitForm>

    <h4>{tx('settings:tools-page.built-in-tools')}</h4>
    <UnitForm<string[]> title={tx('settings:extension-detail.native-tools')} authored={agent?.tools?.builtin ?? undefined} blank={[]} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'native_tools', authored } })}>
      {(value, change) => <>
        <p className={css.hint}>{tx('settings:tools-page.an-absent-selection-grants-no-built-in-tools-an-explicit-empty-s')}</p>
        <CheckboxList label={tx('settings:tools-page.explicit-whitelist')} values={nativeTools} selected={value} change={change} />
      </>}
    </UnitForm>

    <h4>{tx('settings:tools-page.tool-sources')}</h4>
    <p>{tx('settings:tools-page.mcp-servers-and-managed-python-packages-are-inert-until-the-root')}</p>
    {sources.map(id => <UnitForm<SourceToolSelection> key={id} title={tx('settings:extension-detail.source-value', { p0: id })}
      authored={agent?.tools?.sources?.[id] ?? undefined} blank={[]} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'source_tools', id, authored } })}>
      {(value, change) => <Selection label={id} value={value} change={change} />}
    </UnitForm>)}
    <div className={css.actions}>
      <Choice label={tx('settings:tools-page.source-family')} value={family} options={[['mcp', 'MCP'], ['python', tx('settings:copy.managed-python')]]} onChange={setFamily} />
      <TextField label={tx('settings:tools-page.source-identity')} value={source_} change={setSource} />
      <Button disabled={!source_} onClick={() => { setAdded(current => [...current, family === 'python' ? `python:${source_}` : source_]); setSource(''); }}>{tx('settings:tools-page.add-source-selection')}</Button>
    </div>

    <h4>{tx('settings:tools-page.skills')}</h4>
    <p>{tx('settings:tools-page.skill-selection-controls-which-skill-descriptions-are-advertised')}</p>
    <dl><dt>{tx('settings:tools-page.user-skill-root')}</dt><dd>{skillRoots[0]}</dd><dt>{tx('settings:tools-page.workspace-skill-root')}</dt><dd>{skillRoots[1]}</dd></dl>
    <UnitForm<AgentSkillSelection> title={tx('settings:extension-detail.skill-visibility')} authored={agent?.skills ?? undefined} blank={[]} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'skills', authored } })}>
      {(value, change) => <Selection label={tx('settings:extension-detail.visible-skills')} value={value} change={change} />}
    </UnitForm>

    <h4>{tx('settings:tools-page.delegation')}</h4>
    <p>{tx('settings:tools-page.a-defined-named-agent-or-workflow-is-not-a-delegation-target-the')}</p>
    {(['agents', 'workflows'] as const).map(unit => <UnitForm<string[]> key={unit}
      title={unit === 'agents' ? tx('settings:extension-detail.agent-allowlist') : tx('settings:extension-detail.workflow-allowlist')} authored={agent?.[unit] ?? undefined} blank={[]} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit, authored } })}>
      {(value, change) => <Names label={unit} value={value} change={change} />}
    </UnitForm>)}

    <Advanced title={tx('settings:tools-page.advanced-tool-policies')}>
      <p>{tx('settings:tools-page.invocation-policy-governs-execution-ownership-concurrency-and-ap')}</p>
      {policyTools.map(name => <UnitForm<NativePolicyOverrideDocument> key={name} title={tx('settings:tools-page.value-policy', { p0: name })}
        authored={document.native_tools?.[name as NativeTool] ?? undefined} blank={{}} revision={revision}
        mutation={authored => ({ kind: 'config', mutation: { unit: 'native_policy', id: name as NativeTool, authored } })}>
        {(value, change) => <PolicyFields value={value} change={change} />}
      </UnitForm>)}
      <label>{tx('settings:tools-page.mcp-policy-identity')}<input list="mcp-policy-identities" value={mcpPolicy} onChange={event => setMcpPolicy(event.target.value)} /></label>
      <datalist id="mcp-policy-identities">
        {reachableIdentities(scope, document.mcp_tool_policies, resolved?.mcp_tool_policies).map(id => <option key={id} value={id}>{id}</option>)}
      </datalist>
      <Button disabled={!mcpPolicy} onClick={() => selectMcpPolicy(mcpPolicy)}>{tx('settings:tools-page.edit-mcp-policy')}</Button>
      {selectedMcpPolicy && <UnitForm<NativePolicyOverrideDocument> key={selectedMcpPolicy} title={tx('settings:tools-page.mcp-policy-value', { p0: selectedMcpPolicy })}
        authored={document.mcp_tool_policies?.[selectedMcpPolicy] ?? undefined} blank={{}} revision={revision}
        mutation={authored => ({ kind: 'config', mutation: { unit: 'mcp_policy', id: selectedMcpPolicy, authored } })}>
        {(value, change) => <PolicyFields value={value} change={change} />}
      </UnitForm>}
    </Advanced>
    {sourceView(source, scope) === undefined && <p role="alert" className={css.error}>{tx('settings:tools-page.this-scope-has-no-source-view')}</p>}
  </section>;
}

export function PolicyFields({ value, change }: { value: NativePolicyOverrideDocument; change: (value: NativePolicyOverrideDocument) => void }) {
  const tx = useTranslation();
  return <>
    <Choice label={tx('settings:tools-page.execution')} value={value.execution ?? ''} onChange={execution => change({ ...value, execution: (execution || undefined) as NativePolicyOverrideDocument['execution'] })}
      options={[['', tx('settings:copy.domain-default')], ['foreground_only', tx('settings:policy.foreground_only')], ['background_only', tx('settings:policy.background_only')], ['model_selectable', tx('settings:policy.model_selectable')]]} />
    <Choice label={tx('settings:tools-page.concurrency')} value={value.concurrency ?? ''} onChange={concurrency => change({ ...value, concurrency: (concurrency || undefined) as NativePolicyOverrideDocument['concurrency'] })}
      options={[['', tx('settings:copy.domain-default')], ['sequential', tx('settings:policy.sequential')], ['parallel', tx('settings:policy.parallel')]]} />
    <Choice label={tx('settings:tools-page.approval-2')} value={value.approval ?? ''} onChange={approval => change({ ...value, approval: (approval || undefined) as NativePolicyOverrideDocument['approval'] })}
      options={[['', tx('settings:copy.domain-default')], ['never', tx('settings:policy.never')], ['always', tx('settings:policy.always')]]} />
  </>;
}
