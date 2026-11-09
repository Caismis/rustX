import { useEffect, useId, useState } from 'react';
import type { NamedAgentProfileDocument, SourceScope, SourceSettings, SourceToolSelection, WorkflowToolChoice, WorkflowToolSource } from '../../../../../protocol/app-server/v77';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { Switch } from '../../../presentation/primitives/Switch';
import { CheckboxList, Selection, nativeTools } from '../../settings/forms/controls';
import { McpIcon } from '../mcp/McpIcon';
import { ResourceChoices, ScopedChoices, resourceScopes } from './ScopedChoices';
import css from './Subagents.module.css';

type Tools = NonNullable<NamedAgentProfileDocument['tools']>;
type Grants = Pick<NamedAgentProfileDocument, 'tools' | 'workflows'>;
export type LoadMcpTools = (scope: SourceScope, server: string, owner: SourceScope, owns: () => boolean) => Promise<WorkflowToolSource | undefined>;

export function AgentToolFields({ source, scope, value, change, loadMcpTools }: {
  source: SourceSettings; scope: SourceScope; value: Grants; change: (grants: Partial<Grants>) => void;
  loadMcpTools?: LoadMcpTools;
}) {
  const tx = useTranslation();
  const heading = useId();
  const tools = value.tools ?? {};
  const sourceSelections = new Map(Object.entries(tools.sources ?? {}));
  const workflows = value.workflows ?? [];
  const scopes = resourceScopes(scope);
  const entries = source.prospective_resources?.definitions.filter(entry => entry.valid && scopes.includes(entry.location.scope)) ?? [];
  const mcp = entries.filter(entry => entry.family === 'mcp');
  const workflowEntries = entries.filter(entry => entry.family === 'workflow');
  const inherited = scope === 'user' && source.target.kind === 'workspace';

  return <section className={css.section} aria-labelledby={heading}>
    <div className={css.sectionHead}>
      <h2 id={heading}>{tx('resources:subagents.tools')}</h2>
      {!!(tools.builtin?.length || sourceSelections.size || workflows.length) && <Button size="sm"
        onClick={() => change({ tools: { builtin: [], sources: {} }, workflows: [] })}>{tx('resources:subagents.clear-tools')}</Button>}
    </div>
    <div className={css.nativeTools}>
      <CheckboxList label={tx('settings:extension-detail.native-tools')} values={nativeTools}
        selected={tools.builtin ?? []} change={builtin => change({ tools: { ...tools, builtin } })} />
    </div>
    <fieldset className={css.choiceGroup}>
      <legend>{tx('resources:subagents.mcp-tools')}</legend>
      {scopes.map(owner => {
        // An inherited profile still belongs to Global, even if the current
        // Workspace shadows one of its MCP server definitions.
        const servers = inherited ? Object.keys(source.user_mcp.authored ?? {})
          : mcp.filter(entry => entry.location.scope === owner).map(entry => entry.name);
        const revision = (owner === 'workspace' ? source.workspace_mcp : source.user_mcp)?.revision;
        return <ScopedChoices key={owner} owner={owner}>
          {servers.length ? <div className={css.serverList}>
            {servers.map(name => <McpServerTools key={JSON.stringify([scope, name, revision])}
              scope={scope} owner={owner} name={name} selected={sourceSelections.get(name)} loadMcpTools={loadMcpTools}
              change={next => {
                const sources = new Map(sourceSelections);
                if (next === undefined) sources.delete(name);
                else sources.set(name, next);
                change({ tools: { ...tools, sources: Object.fromEntries(sources) } });
              }} />)}
          </div> : <p>{tx('resources:subagents.no-mcp-tools')}</p>}
        </ScopedChoices>;
      })}
    </fieldset>
    <fieldset className={css.choiceGroup}>
      <legend>{tx('resources:subagents.workflows')}</legend>
      {workflowEntries.length > 0 && <p>{tx('resources:subagents.workflows-help')}</p>}
      {scopes.map(owner => {
        const options = workflowEntries.filter(entry => entry.location.scope === owner).map(entry => ({ name: entry.name }));
        const names = new Set(options.map(entry => entry.name));
        return <ScopedChoices key={owner} owner={owner}>
          {options.length ? <ResourceChoices label={tx(owner === 'workspace' ? 'resources:scope.workspace' : 'resources:scope.global')}
            options={options} selected={workflows.filter(id => names.has(id))}
            change={checked => change({ workflows: [...workflows.filter(id => !names.has(id)), ...checked] })} />
            : <p>{tx('resources:subagents.no-workflows')}</p>}
        </ScopedChoices>;
      })}
    </fieldset>
  </section>;
}

function McpServerTools({ scope, owner, name, selected, change, loadMcpTools }: {
  scope: SourceScope; owner: SourceScope; name: string; selected?: SourceToolSelection;
  change: (value: SourceToolSelection | undefined) => void; loadMcpTools?: LoadMcpTools;
}) {
  const tx = useTranslation();
  const [attempt, setAttempt] = useState(0);
  const [result, setResult] = useState<{ attempt: number; tools?: WorkflowToolChoice[] }>();
  const current = result?.attempt === attempt ? result : undefined;
  const discovered = current?.tools;
  const failed = current !== undefined && !discovered;
  useEffect(() => {
    if (!loadMcpTools || current) return;
    let active = true;
    void loadMcpTools(scope, name, owner, () => active).then(
      listed => {
        if (!active) return;
        const tools = listed?.kind === 'mcp' && listed.scope === owner && listed.state === 'listed' ? listed.tools : undefined;
        setResult({ attempt, tools });
      },
      () => { if (active) setResult({ attempt }); },
    );
    return () => { active = false; };
  }, [attempt, current, loadMcpTools, name, owner, scope]);
  const all = selected === 'all';
  const exact = all ? (discovered ?? []).map(tool => tool.name) : selected ?? [];
  const options = new Map<string, { name: string; description?: string }>(discovered?.map(tool => [tool.name, tool]));
  for (const tool of exact) if (!options.has(tool)) options.set(tool, { name: tool });

  return <div className={css.toolServer}>
    <div className={css.toolServerHead}>
      <McpIcon />
      <strong>{name}</strong>
      <div className={css.toolSwitch}>
        <span>{tx('resources:subagents.all-mcp-tools')}</span>
        <Switch label={tx('resources:subagents.all-mcp-tools-for', { p0: name })} checked={all}
          onChange={checked => change(checked ? 'all' : undefined)} />
      </div>
    </div>
    {options.size > 0 && <ResourceChoices label={tx('resources:subagents.mcp-tool-list', { p0: name })}
      options={[...options.values()]} selected={exact} change={checked => change(checked.length ? checked : undefined)} />}
    {failed ? <div className={css.toolStatus}>
      <p role="status">{tx('resources:subagents.mcp-tools-failed')}</p>
      <Button size="sm" disabled={!loadMcpTools} aria-label={tx('resources:subagents.retry-mcp-tools-for', { p0: name })}
        onClick={() => setAttempt(value => value + 1)}>{tx('resources:subagents.retry-mcp-tools')}</Button>
    </div> : !current && loadMcpTools ? <div className={css.toolStatus}><p role="status">{tx('resources:subagents.loading-mcp-tools')}</p></div>
      : discovered?.length === 0 && <div className={css.toolStatus}><p>{tx('resources:subagents.mcp-empty')}</p></div>}
  </div>;
}

/** Already authored Managed Python selections still use their native source
 * semantics. Keep this independent of the MCP catalog. */
export function ManagedToolSelections({ tools, change }: { tools?: Tools | null; change: (tools: Tools) => void }) {
  const tx = useTranslation();
  const sources = Object.entries(tools?.sources ?? {}).filter(([id]) => id.startsWith('python:'));
  return sources.map(([id, selection]) => <div key={id}>
    <Selection label={id} value={selection} change={next => change({ ...tools, sources: { ...tools?.sources, [id]: next } })} />
    <Button type="button" size="sm" onClick={() => {
      const next = { ...tools?.sources };
      delete next[id];
      change({ ...tools, sources: next });
    }}>{tx('settings:extension-detail.remove-source-selection')} {id}</Button>
  </div>);
}
