import type { SourceMutation, SourceScope, SourceSettings } from '../../../../../protocol/app-server/v77';
import { useTranslation } from '../../../locale/react';
import { Switch } from '../../../presentation/primitives/Switch';
import { Button } from '../../../presentation/primitives/Button';
import { UnitOutcomeNotice, useUnitEditing } from '../../settings/forms/bridge';
import { toolSourceId } from '../../settings/capability';
import css from './ResourceCatalog.module.css';

type Selection = 'all' | string[];
/** Quick selection edits the same semantic unit as the full permission editor. */
export function ResourceAvailability({ family, name, valid, source, scope, revision, skillNames }: {
  family: 'mcp' | 'skill' | 'agent'; name: string; valid: boolean | undefined;
  source: SourceSettings; scope: SourceScope; revision: string; skillNames?: string[];
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
  const selected = selection === 'all' || (family === 'mcp' ? names.length > 0 : names.includes(name));
  const review = !!unit.draft || unit.reviewNeeded;
  return <div className={css.availability} aria-busy={unit.busy || unit.awaitingObservation}>
    <Switch label={tx('settings:catalog.enable', { name })} checked={selected}
      disabled={review || !unit.writable || !unit.admitted || unit.busy || valid === false && !selected}
      onChange={enabled => unit.apply(family === 'mcp' ? enabled ? 'all' : [] : enabled ? [...new Set([...names, name])] : names.filter(item => item !== name))}/>
    {unit.reviewNeeded && <Button size="sm" onClick={unit.review}>{tx('settings:bridge.use-reviewed-revision')}</Button>}
    {unit.draft && !unit.busy && !unit.awaitingObservation && <Button size="sm" onClick={unit.discard}>{tx('settings:bridge.discard-draft')}</Button>}
    <UnitOutcomeNotice title={name} outcome={unit.outcome}/>
  </div>;
}
