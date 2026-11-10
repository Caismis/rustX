import type { SourceMutation, SourceScope, SourceSettings } from '../../../../../protocol/app-server/v77';
import { useTranslation } from '../../../locale/react';
import { Switch } from '../../../presentation/primitives/Switch';
import { Button } from '../../../presentation/primitives/Button';
import { UnitOutcomeNotice, useUnitEditing } from '../../settings/forms/bridge';
import { Choice } from '../../settings/primitives/aria';
import { SettingsNotice } from '../../settings/SettingsFeedback';
import { toolSourceId } from '../../settings/capability';
import css from './ResourceCatalog.module.css';

type Selection = 'all' | string[];

/** Root-agent policy is independent of which skills happen to be discovered
 * today. Per-card toggles author exact lists; this control also exposes the
 * dynamic all policy and removal of a Workspace override. */
export function SkillSelectionPolicy({ source, scope, revision, skillNames }: {
  source: SourceSettings; scope: SourceScope; revision: string; skillNames?: string[];
}) {
  const tx = useTranslation();
  const authored = (scope === 'user' ? source.user : source.workspace)?.authored?.agent?.skills ?? undefined;
  const unit = useUnitEditing<Selection>({ authored, blank: [], revision,
    mutation: value => ({ kind: 'config', mutation: { unit: 'skills', authored: value } }) });
  const selection = unit.draft ? unit.displayed : authored ?? source.resolved?.agent?.skills ?? 'all';
  const mode = scope === 'workspace' && !unit.authoredPresent && !unit.draft ? 'inherit'
    : selection === 'all' ? 'all' : selection.length ? 'selected' : 'none';
  const blocked = unit.draft || unit.reviewNeeded || !unit.writable || !unit.admitted || unit.busy;
  const canFreeze = selection !== 'all' || skillNames !== undefined;
  return <section className={css.skillPolicy} aria-label={tx('resources:skills.selection')} aria-busy={unit.busy || unit.awaitingObservation}>
    <Choice label={tx('resources:skills.selection')} value={mode} disabled={blocked}
      disabledKeys={canFreeze ? undefined : ['selected']}
      options={[
        ...(scope === 'workspace' ? [['inherit', tx('resources:skills.inherit')] as const] : []),
        ['all', tx('resources:skills.all')], ['selected', tx('resources:skills.selected')], ['none', tx('settings:copy.none')],
      ]} onChange={next => {
        if (blocked || next === 'selected' && !canFreeze) return;
        if (next === 'inherit') { if (unit.authoredPresent) unit.submit(true); }
        else unit.apply(next === 'all' ? 'all' : next === 'none' ? [] : selection === 'all' ? skillNames! : selection);
      }} />
    <div className={css.availability}>
      {unit.reviewNeeded && <Button size="sm" onClick={unit.review}>{tx('settings:bridge.use-reviewed-revision')}</Button>}
      {unit.draft && !unit.busy && !unit.awaitingObservation && <Button size="sm" onClick={unit.discard}>{tx('settings:bridge.discard-draft')}</Button>}
    </div>
    <SettingsNotice text={unit.reviewNeeded && unit.outcome.kind !== 'conflict' ? tx(unit.committed && !unit.draft ? 'settings:forms.committed-changed-elsewhere' : 'settings:forms.changed-elsewhere') : undefined} />
    <UnitOutcomeNotice title={tx('resources:skills.title')} outcome={unit.outcome}/>
  </section>;
}

/** Quick selection edits the same semantic unit as the full permission editor. */
export function ResourceAvailability({ family, name, valid, source, scope, revision, skillNames, skillEnabledByDefault }: {
  family: 'mcp' | 'skill' | 'agent'; name: string; valid: boolean | undefined;
  source: SourceSettings; scope: SourceScope; revision: string; skillNames?: string[];
  skillEnabledByDefault?: boolean;
}) {
  const tx = useTranslation();
  const agent = (scope === 'user' ? source.user : source.workspace)?.authored?.agent;
  const sourceId = toolSourceId(family, name);
  const authored = family === 'skill' ? agent?.skills : family === 'agent' ? agent?.agents : agent?.tools?.sources?.[sourceId];
  const mutation = (value: Selection | null): SourceMutation => family === 'skill'
    ? { kind: 'config', mutation: { unit: 'skills', authored: value } }
    : family === 'agent' ? { kind: 'config', mutation: { unit: 'agents', authored: value as string[] | null } }
      : { kind: 'config', mutation: { unit: 'source_tools', id: sourceId, authored: value } };
  const unit = useUnitEditing<Selection>({ authored: authored ?? undefined, blank: [], revision, mutation });
  const main = source.prospective_resources?.main;
  const sourceTools = main?.tool_selection.filter(tool => tool.origin !== 'builtin' && tool.source_id === sourceId) ?? [];
  const sourceSelection: Selection = sourceTools.some(tool => tool.origin === 'all') ? 'all' : sourceTools.flatMap(tool => tool.origin === 'source' ? [tool.name] : []);
  const effective = family === 'skill' ? source.resolved?.agent?.skills ?? 'all'
    : family === 'agent' ? source.resolved?.agent?.agents ?? main?.agents
      : source.resolved?.agent?.tools?.sources?.[sourceId] ?? sourceSelection;
  const selection = unit.draft ? unit.displayed : authored ?? effective ?? [];
  const names = Array.isArray(selection) ? selection : skillNames ?? (source.prospective_resources?.definitions ?? []).filter(row => row.family === family && row.valid).map(row => row.name);
  const selected = selection === 'all' ? skillEnabledByDefault ?? true : family === 'mcp' ? names.length > 0 : names.includes(name);
  const review = !!unit.draft || unit.reviewNeeded;
  return <div className={css.availability} aria-busy={unit.busy || unit.awaitingObservation}>
    <Switch label={tx('settings:catalog.enable', { name })} checked={selected}
      disabled={review || !unit.writable || !unit.admitted || unit.busy || valid === false && !selected
        || family === 'skill' && selection === 'all' && skillNames === undefined}
      onChange={enabled => unit.apply(family === 'mcp' ? enabled ? 'all' : [] : enabled ? [...new Set([...names, name])] : names.filter(item => item !== name))}/>
    {unit.reviewNeeded && <Button size="sm" onClick={unit.review}>{tx('settings:bridge.use-reviewed-revision')}</Button>}
    {unit.draft && !unit.busy && !unit.awaitingObservation && <Button size="sm" onClick={unit.discard}>{tx('settings:bridge.discard-draft')}</Button>}
    <UnitOutcomeNotice title={name} outcome={unit.outcome}/>
  </div>;
}
