import { useState } from 'react';
import type { NamedAgentProfileDocument, ResourceDefinition, SourceScope, SourceSettings, SourceToolSelection, WorkflowToolSource } from '../../../../../protocol/app-server/v77';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { Switch } from '../../../presentation/primitives/Switch';
import { CheckboxList, Selection, nativeTools } from '../../settings/forms/controls';
import { Choice } from '../../settings/primitives/aria';
import { SettingsNotice } from '../../settings/SettingsFeedback';
import css from './Subagents.module.css';

type Tools = NonNullable<NamedAgentProfileDocument['tools']>;
type Grants = Pick<NamedAgentProfileDocument, 'tools' | 'workflows'>;
export type LoadMcpTools = (server: string) => Promise<WorkflowToolSource | undefined>;

export function AgentToolFields({ source, scope, value, change, loadMcpTools }: {
  source: SourceSettings; scope: SourceScope; value: Grants; change: (grants: Partial<Grants>) => void;
  loadMcpTools?: LoadMcpTools;
}) {
  const tx = useTranslation();
  const [customizing, setCustomizing] = useState(false);
  const tools = value.tools ?? {};
  const sourceSelections = new Map(Object.entries(tools.sources ?? {}));
  const workflows = value.workflows ?? [];
  const selected = !!tools.builtin?.length || !!sourceSelections.size || !!workflows.length;
  const scopes: readonly SourceScope[] = scope === 'workspace' ? ['workspace', 'user'] : ['user'];
  const entries = source.prospective_resources?.definitions.filter(entry => entry.valid && scopes.includes(entry.location.scope)) ?? [];
  const mcp = entries.filter(entry => entry.family === 'mcp');
  const workflowEntries = entries.filter(entry => entry.family === 'workflow');
  const scopeLabel = (owner: SourceScope) => tx(owner === 'workspace' ? 'resources:scope.workspace' : 'resources:scope.global');

  return <div className={css.tools}>
    <Choice label={tx('resources:subagents.tools')} value={customizing || selected ? 'selected' : 'none'}
      options={[['none', tx('resources:subagents.no-tools')], ['selected', tx('resources:subagents.selected-tools')]]}
      description={tx('resources:subagents.tools-help')}
      onChange={mode => {
        setCustomizing(mode === 'selected');
        if (mode === 'none' && selected) change({ tools: { builtin: [], sources: {} }, workflows: [] });
      }} />
    {(customizing || selected) && <div className={css.toolChoices}>
      <div className={css.nativeTools}>
        <CheckboxList label={tx('settings:extension-detail.native-tools')} values={nativeTools}
          selected={tools.builtin ?? []} change={builtin => change({ tools: { ...tools, builtin } })} />
      </div>
      <fieldset className={css.scopedTools}>
        <legend>{tx('resources:subagents.mcp-tools')}</legend>
        {scopes.map(owner => {
          const servers = mcp.filter(entry => entry.location.scope === owner);
          const revision = (owner === 'workspace' ? source.workspace_mcp : source.user_mcp)?.revision;
          return <section key={owner} aria-label={scopeLabel(owner)} className={css.toolScope}>
            <h4>{scopeLabel(owner)}</h4>
            {servers.length ? servers.map(entry => <McpServerTools key={JSON.stringify([entry.name, revision])}
              entry={entry} selected={sourceSelections.get(entry.name)} loadMcpTools={loadMcpTools}
              change={next => {
                const sources = new Map(sourceSelections);
                if (next === undefined) sources.delete(entry.name);
                else sources.set(entry.name, next);
                change({ tools: { ...tools, sources: Object.fromEntries(sources) } });
              }} />) : <p>{tx('resources:subagents.no-mcp-tools')}</p>}
          </section>;
        })}
      </fieldset>
      <fieldset className={css.scopedTools}>
        <legend>{tx('resources:subagents.workflows')}</legend>
        {workflowEntries.length > 0 && <p>{tx('resources:subagents.workflows-help')}</p>}
        {scopes.map(owner => {
          const choices = workflowEntries.filter(entry => entry.location.scope === owner).map(entry => entry.name);
          return <div key={owner} className={`${css.toolScope} ${css.toolOptions}`}>
            {choices.length ? <CheckboxList label={scopeLabel(owner)} values={choices} selected={workflows.filter(id => choices.includes(id))}
              change={checked => change({ workflows: [...workflows.filter(id => !choices.includes(id)), ...checked] })} />
              : <section aria-label={scopeLabel(owner)}><h4>{scopeLabel(owner)}</h4><p>{tx('resources:subagents.no-workflows')}</p></section>}
          </div>;
        })}
      </fieldset>
    </div>}
  </div>;
}

function McpServerTools({ entry, selected, change, loadMcpTools }: {
  entry: ResourceDefinition; selected?: SourceToolSelection;
  change: (value: SourceToolSelection | undefined) => void; loadMcpTools?: LoadMcpTools;
}) {
  const tx = useTranslation();
  const [status, setStatus] = useState<'idle' | 'loading' | 'listed' | 'failed'>('idle');
  const [names, setNames] = useState<string[]>([]);
  const all = selected === 'all';
  const exact = all ? names : selected ?? [];
  const label = tx('resources:subagents.mcp-tool-list', { p0: entry.name });
  const action = tx(status === 'loading' ? 'resources:subagents.loading-mcp-tools'
    : status === 'failed' ? 'resources:subagents.retry-mcp-tools'
    : status === 'listed' ? 'resources:refresh' : 'resources:subagents.view-mcp-tools');
  const load = async () => {
    if (!loadMcpTools || status === 'loading') return;
    setStatus('loading');
    try {
      const listed = await loadMcpTools(entry.name);
      if (listed?.kind !== 'mcp' || listed.scope !== entry.location.scope || listed.state !== 'listed') {
        setStatus('failed');
        return;
      }
      setNames(listed.tools.map(tool => tool.name));
      setStatus('listed');
    } catch {
      setStatus('failed');
    }
  };
  return <div className={css.toolServer}>
    <div className={css.toolServerHead}>
      <strong>{entry.name}</strong>
      <div className={css.toolSwitch}>
        <span>{tx('resources:subagents.all-mcp-tools')}</span>
        <Switch label={tx('resources:subagents.all-mcp-tools-for', { p0: entry.name })} checked={all}
          onChange={checked => change(checked ? 'all' : undefined)} />
      </div>
      {loadMcpTools && <Button type="button" size="sm" variant="outline" disabled={status === 'loading'}
        aria-label={`${action} ${entry.name}`} aria-busy={status === 'loading'} onClick={() => void load()}>{action}</Button>}
    </div>
    <SettingsNotice text={status === 'failed' ? tx('resources:subagents.mcp-tools-failed', { p0: entry.name }) : undefined} />
    {(names.length > 0 || exact.length > 0) && <div className={css.toolOptions}>
      <CheckboxList label={label} values={names} selected={exact}
        change={checked => change(checked.length ? checked : undefined)} />
    </div>}
    {status === 'listed' && !names.length && <p>{tx('resources:subagents.mcp-empty')}</p>}
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
