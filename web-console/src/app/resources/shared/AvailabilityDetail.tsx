/* Copyright (c) 2026 DeepSeek. MIT. Adapted resource controls; see PROVENANCE.md. */
import { useContext, useEffect, useState } from 'react';
import type { SourceSettings, SourceScope, SourceToolSelection, WorkflowToolChoice } from '../../../../../protocol/app-server/v80';
import { useTranslation } from '../../../locale/react';
import settingsCss from '../../../presentation/settings/SettingsContent.module.css';
import { toolSourceId } from '../../settings/capability';
import { UnitForm, UnitReset } from '../../settings/forms/bridge';
import { ConfigUnavailable } from '../../settings/forms/ConfigUnavailable';
import { Selection } from '../../settings/forms/controls';
import { Choice } from '../../settings/primitives/aria';
import { ResourceChoices } from '../subagents/ScopedChoices';
import subagentsCss from '../subagents/Subagents.module.css';

/** Root Agent tool availability. Server-level approvals live in McpPermissionDetail. */
export function AvailabilityDetail({ source, scope, revision, family, name, tools, heading, saveLabel, help }: {
  source: SourceSettings; scope: SourceScope; revision?: string; family: 'mcp' | 'managed_python'; name: string;
  tools?: readonly WorkflowToolChoice[]; heading?: string; saveLabel?: string; help?: string;
}) {
  const tx = useTranslation();
  if (revision === undefined) return <ConfigUnavailable />;
  const document = (scope === 'user' ? source.user : source.workspace)?.authored;
  const id = toolSourceId(family, name);
  const title = heading ?? tx('settings:extension-detail.availability-to-the-root-agent');
  return <section aria-label={title}>
    <h4>{title}</h4>
    {help && <p className={settingsCss.hint}>{help}</p>}
    <UnitForm<SourceToolSelection> title={tx('settings:extension-detail.source-value', { p0: id })}
      authored={document?.agent?.tools?.sources?.[id] ?? undefined} blank={[]} revision={revision} saveLabel={saveLabel}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'source_tools', id, authored } })}>
      {(value, change) => tools?.length
        ? <ToolChoices label={id} value={value} change={change} tools={tools} />
        : <Selection label={id} value={value} change={change} />}
    </UnitForm>
  </section>;
}

function ToolChoices({ label, value, change, tools }: {
  label: string; value: SourceToolSelection; change: (value: SourceToolSelection) => void; tools: readonly WorkflowToolChoice[];
}) {
  const tx = useTranslation();
  const resets = useContext(UnitReset);
  const [choosingExact, setChoosingExact] = useState(false);
  useEffect(() => { setChoosingExact(false); }, [resets]);
  const selected = Array.isArray(value) ? value : [];
  const mode = value === 'all' ? 'all' : selected.length || choosingExact ? 'exact' : 'none';
  const options = new Map(tools.map(({ name, description }) => [name, { name, description }]));
  for (const name of selected) {
    if (!options.has(name)) options.set(name, { name, description: tx('resources:mcp.tool-not-listed') });
  }
  return <>
    <Choice label={tx('settings:controls.selection')} value={mode}
      options={[
        ['none', tx('settings:copy.none')],
        ['all', tx('settings:copy.all')],
        ['exact', tx('resources:mcp.selected-tools')],
      ]}
      onChange={next => {
        setChoosingExact(next === 'exact');
        // Opening the picker never grants tools. An exact list can be empty.
        if (next === 'exact') {
          if (value === 'all') change([]);
        } else change(next === 'all' ? 'all' : []);
      }} />
    {mode === 'exact' && <div className={subagentsCss.form}>
      <ResourceChoices label={tx('resources:subagents.mcp-tool-list', { p0: label })}
        options={[...options.values()]} selected={selected}
        change={next => { setChoosingExact(true); change(next); }} />
    </div>}
    {mode === 'all' && <p className={settingsCss.hint}>{tx('resources:mcp.all-tools')}</p>}
  </>;
}
