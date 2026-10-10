import type { SourceMutation, SourceScope, SourceSettings } from '../../../../../protocol/app-server/v40';
import { useTranslation } from '../../../locale/react';
import { Switch } from '../../../presentation/primitives/Switch';
import { UnitOutcomeNotice, useUnitEditing } from '../forms/bridge';
import { toolSourceId } from '../capability';
import css from './ResourceCatalog.module.css';

type Selection = 'all' | string[];
/** Quick selection edits the same semantic unit as the full permission editor. */
export function ResourceAvailability({ family, name, valid, source, scope, revision, inspect }: {
  family: 'mcp' | 'skill' | 'agent'; name: string; valid: boolean | undefined;
  source: SourceSettings; scope: SourceScope; revision: string; inspect?: () => void;
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
  const names = Array.isArray(unit.displayed) ? unit.displayed : [];
  const selected = unit.displayed === 'all' || (family === 'mcp' ? names.length > 0 : names.includes(name));
  // Do not replace an outstanding draft, a partial MCP tool grant, or an open
  // Skill wildcard with a different permission policy through a binary toggle.
  const review = unit.draft || unit.reviewNeeded || (family === 'skill' && unit.displayed === 'all')
    || (family === 'mcp' && names.length > 0);
  if (review && !unit.busy && inspect) return <button type="button" className={css.manage} onClick={inspect}>{tx('settings:catalog.review-selection')}</button>;
  return <div className={css.availability}>
    <Switch label={tx('settings:catalog.enable', { name })} checked={selected}
      disabled={!!review || !unit.writable || !unit.admitted || unit.busy || valid === false}
      onChange={enabled => unit.apply(family === 'mcp' ? enabled ? 'all' : [] : enabled ? [...new Set([...names, name])] : names.filter(item => item !== name))}/>
    {inspect && unit.busy && <span role="status" className={css.count}>{tx('settings:catalog.saving')}</span>}
    {!inspect && <UnitOutcomeNotice title={name} outcome={unit.outcome}/>}
  </div>;
}
