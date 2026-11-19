import type { McpWrite, ResourcePermissionSet, WorkflowToolChoice } from '../../../../../protocol/app-server/v80';
import { useTranslation } from '../../../locale/react';
import { UnitForm } from '../../settings/forms/bridge';
import { ResourcePermissionFields, blankResourcePermissions } from '../../settings/forms/ResourcePermissionFields';
import css from './McpPage.module.css';

const presets = ['allow', 'ask', 'deny'] as const;
const modes = ['plan', 'automatic', 'full_access'] as const;
const modeLabels = {
  plan: 'settings:permission.plan',
  automatic: 'settings:permission.automatic',
  full_access: 'settings:permission.full-access',
} as const;
type Approval = { rules: ResourcePermissionSet | null };

export function McpToolCard({ tool, server, authored, revision }: {
  tool: WorkflowToolChoice;
  server: string;
  authored: McpWrite;
  revision: string;
}) {
  const tx = useTranslation();
  const overrides = authored.definition.tools;
  const rules = overrides && Object.hasOwn(overrides, tool.name) ? overrides[tool.name].permissions ?? null : null;
  return <li className={css.toolCard}>
    <UnitForm<Approval> title={tx('resources:mcp.tool-rule-for', { name: tool.name })}
      authored={{ rules }} blank={{ rules: null }} revision={revision}
      draftKey={JSON.stringify(['mcp-tool-rule', server, tool.name])} removable={false}
      saveLabel={tx('resources:mcp.save-tool-rule')}
      mutation={value => {
        if (value === null) return { kind: 'mcp', id: server, authored: null };
        const tools = { ...overrides };
        if (value.rules === null) delete tools[tool.name];
        const nextTools = value.rules === null ? tools : { ...tools, [tool.name]: { permissions: value.rules } };
        return {
          kind: 'mcp', id: server,
          authored: { ...authored, definition: { ...authored.definition, tools: Object.keys(nextTools).length > 0 ? nextTools : undefined } },
        };
      }}>
      {(value, change) => {
        const selected = value.rules;
        const policy = selected === null ? 'inherit'
          : presets.find(preset => modes.every(mode => selected[mode] === preset)) ?? 'custom';
        const effective = selected ?? authored.definition.permissions ?? blankResourcePermissions;
        return <>
          <div className={css.toolCardHeading}>
            <strong>{tool.name}</strong>
            <span className={css.ruleBadge} data-overridden={selected !== null || undefined}>
              {tx(selected === null ? 'resources:mcp.rule-inherited' : 'resources:mcp.rule-overridden')}
            </span>
          </div>
          {tool.description && <p>{tool.description}</p>}
          <label className={css.policyChoice}>
            {tx('resources:mcp.tool-approval')}
            <select value={policy} onChange={event => {
              const next = event.target.value;
              const preset = presets.find(option => option === next);
              if (preset) change({ rules: { plan: preset, automatic: preset, full_access: preset } });
              else if (next === 'inherit') change({ rules: null });
            }}>
              <option value="inherit">{tx('resources:mcp.inherit-server')}</option>
              {presets.map(preset => <option key={preset} value={preset}>{tx(`settings:permission.${preset}`)}</option>)}
              {policy === 'custom' && <option value="custom" disabled>{tx('resources:mcp.custom-rule')}</option>}
            </select>
          </label>
          <dl className={css.modeSummary}>
            {modes.map(mode => <div key={mode}>
              <dt>{tx(modeLabels[mode])}</dt>
              <dd>{tx(effective[mode] === 'default' ? 'resources:mcp.built-in-rule' : `settings:permission.${effective[mode]}`)}</dd>
            </div>)}
          </dl>
          <details className={css.modeDetails}>
            <summary>{tx('resources:mcp.per-mode-rules')}</summary>
            <p>{tx('resources:mcp.custom-rule-help')}</p>
            <div className={css.modeFields}>
              <ResourcePermissionFields value={effective} change={rules => change({ rules })} />
            </div>
          </details>
        </>;
      }}
    </UnitForm>
  </li>;
}
