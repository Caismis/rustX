import { useState } from 'react';
import type {
  AgentSkillSelection, ApprovalMode, NativePolicyOverrideDocument, NativeTool,
  RuntimeLayer, SourceScope, SourceSettings, SourceToolSelection,
} from '../../../../../protocol/app-server/v24';
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
  return <section aria-label="Tools & Permissions">
    <h3>Tools &amp; Permissions</h3>
    <p>What the root Agent may use, and under what policy. Defining a resource never grants it: each grant below is its own decision, saved on its own.</p>

    <h4>Approval</h4>
    <UnitForm<ApprovalMode> title="Approval mode" authored={document.approval_mode ?? undefined} blank="policy" revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'approval', authored } })}>
      {(value, change) => <Choice label="Approval mode" value={value}
        options={[['policy', 'Policy'], ['full_access', 'Full access']]} onChange={change} />}
    </UnitForm>

    <h4>Built-in Tools</h4>
    <UnitForm<string[]> title="Native Tools" authored={agent?.tools?.builtin ?? undefined} blank={[]} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'native_tools', authored } })}>
      {(value, change) => <>
        <p className={css.hint}>An absent selection grants no built-in Tools. An explicit empty selection is an authored decision to grant none.</p>
        <CheckboxList label="Explicit whitelist" values={nativeTools} selected={value} change={change} />
      </>}
    </UnitForm>

    <h4>Tool sources</h4>
    <p>MCP servers and Managed Python packages are inert until the root Agent selects them. Each source is selected atomically: all of its Tools, an exact list, or none.</p>
    {sources.map(id => <UnitForm<SourceToolSelection> key={id} title={`Source ${id}`}
      authored={agent?.tools?.sources?.[id] ?? undefined} blank={[]} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'source_tools', id, authored } })}>
      {(value, change) => <Selection label={id} value={value} change={change} />}
    </UnitForm>)}
    <div className={css.actions}>
      <Choice label="Source family" value={family} options={[['mcp', 'MCP'], ['python', 'Managed Python']]} onChange={setFamily} />
      <TextField label="Source identity" value={source_} change={setSource} />
      <Button disabled={!source_} onClick={() => { setAdded(current => [...current, family === 'python' ? `python:${source_}` : source_]); setSource(''); }}>Add source selection</Button>
    </div>

    <h4>Skills</h4>
    <p>Skill selection controls which Skill descriptions are advertised in the prompt. It is native prompt and resource selection, not a filesystem ACL and not a security sandbox: Tools keep their ordinary read behavior either way.</p>
    <dl><dt>User Skill root</dt><dd>{skillRoots[0]}</dd><dt>Workspace Skill root</dt><dd>{skillRoots[1]}</dd></dl>
    <UnitForm<AgentSkillSelection> title="Skill visibility" authored={agent?.skills ?? undefined} blank={[]} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'skills', authored } })}>
      {(value, change) => <Selection label="Visible Skills" value={value} change={change} />}
    </UnitForm>

    <h4>Delegation</h4>
    <p>A defined named Agent or Workflow is not a delegation target. These allowlists are what makes one available to the root Agent, and each is an independent mutation from the definition itself.</p>
    {(['agents', 'workflows'] as const).map(unit => <UnitForm<string[]> key={unit}
      title={unit === 'agents' ? 'Agent allowlist' : 'Workflow allowlist'} authored={agent?.[unit] ?? undefined} blank={[]} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit, authored } })}>
      {(value, change) => <Names label={unit} value={value} change={change} />}
    </UnitForm>)}

    <Advanced title="Advanced Tool policies">
      <p>Invocation policy governs execution ownership, concurrency and approval independently of which Tools the Agent may use. Each policy is replaced as one complete object.</p>
      {policyTools.map(name => <UnitForm<NativePolicyOverrideDocument> key={name} title={`${name} policy`}
        authored={document.native_tools?.[name as NativeTool] ?? undefined} blank={{}} revision={revision}
        mutation={authored => ({ kind: 'config', mutation: { unit: 'native_policy', id: name as NativeTool, authored } })}>
        {(value, change) => <PolicyFields value={value} change={change} />}
      </UnitForm>)}
      <label>MCP policy identity<input list="mcp-policy-identities" value={mcpPolicy} onChange={event => setMcpPolicy(event.target.value)} /></label>
      <datalist id="mcp-policy-identities">
        {reachableIdentities(scope, document.mcp_tool_policies, resolved?.mcp_tool_policies).map(id => <option key={id}>{id}</option>)}
      </datalist>
      <Button disabled={!mcpPolicy} onClick={() => selectMcpPolicy(mcpPolicy)}>Edit MCP policy</Button>
      {selectedMcpPolicy && <UnitForm<NativePolicyOverrideDocument> key={selectedMcpPolicy} title={`MCP policy ${selectedMcpPolicy}`}
        authored={document.mcp_tool_policies?.[selectedMcpPolicy] ?? undefined} blank={{}} revision={revision}
        mutation={authored => ({ kind: 'config', mutation: { unit: 'mcp_policy', id: selectedMcpPolicy, authored } })}>
        {(value, change) => <PolicyFields value={value} change={change} />}
      </UnitForm>}
    </Advanced>
    {sourceView(source, scope) === undefined && <p role="alert" className={css.error}>This scope has no source view.</p>}
  </section>;
}

export function PolicyFields({ value, change }: { value: NativePolicyOverrideDocument; change: (value: NativePolicyOverrideDocument) => void }) {
  return <>
    <Choice label="execution" value={value.execution ?? ''} onChange={execution => change({ ...value, execution: (execution || undefined) as NativePolicyOverrideDocument['execution'] })}
      options={[['', 'Domain default'], ['foreground_only', 'foreground_only'], ['background_only', 'background_only'], ['model_selectable', 'model_selectable']]} />
    <Choice label="concurrency" value={value.concurrency ?? ''} onChange={concurrency => change({ ...value, concurrency: (concurrency || undefined) as NativePolicyOverrideDocument['concurrency'] })}
      options={[['', 'Domain default'], ['sequential', 'sequential'], ['parallel', 'parallel']]} />
    <Choice label="approval" value={value.approval ?? ''} onChange={approval => change({ ...value, approval: (approval || undefined) as NativePolicyOverrideDocument['approval'] })}
      options={[['', 'Domain default'], ['never', 'never'], ['always', 'always']]} />
  </>;
}
