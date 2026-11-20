import { useId, useState, type ReactNode } from 'react';
import type { SourceScope } from '../../../../../protocol/app-server/v80';
import { useTranslation } from '../../../locale/react';
import { IconChevronDownOutline14, IconSearchOutline16 } from '../../../presentation/primitives/icons';
import css from './Subagents.module.css';

export function resourceScopes(scope: SourceScope): readonly SourceScope[] {
  return scope === 'workspace' ? ['workspace', 'user'] : ['user'];
}

export function ScopedChoices({ owner, children }: { owner: SourceScope; children: ReactNode }) {
  const tx = useTranslation();
  const label = tx(owner === 'workspace' ? 'resources:scope.workspace' : 'resources:scope.global');
  return <section aria-label={label} className={css.scopedChoices}>
    <span className={css.scopeHeading}>{label}</span>
    {children}
  </section>;
}

type ResourceOption = { name: string; description?: string };

function ToolChoiceRow({ option, checked, onChange }: {
  option: ResourceOption; checked: boolean; onChange: (checked: boolean) => void;
}) {
  const tx = useTranslation();
  const descriptionId = useId();
  const [expanded, setExpanded] = useState(false);
  const { name, description } = option;
  return <div className={css.toolChoiceRow} data-expanded={expanded || undefined}>
    <label className={css.toolChoiceLabel}>
      <input type="checkbox" checked={checked} aria-label={name}
        onChange={event => onChange(event.target.checked)} />
      <span className={css.toolChoiceText}>
        <span className={css.toolChoiceName}>{name}</span>
        {description && <span id={descriptionId} className={css.toolChoiceDescription}>{description}</span>}
      </span>
    </label>
    {description && <button type="button" className={css.toolChoiceDetails} aria-expanded={expanded} aria-controls={descriptionId}
      aria-label={tx('resources:subagents.tool-description', { name })} onClick={() => setExpanded(!expanded)}>
      <IconChevronDownOutline14 />
    </button>}
  </div>;
}

export function ResourceChoices({ label, options, selected, change, compactTools = false }: {
  label: string;
  options: readonly ResourceOption[];
  selected: readonly string[];
  change: (next: string[]) => void;
  compactTools?: boolean;
}) {
  const tx = useTranslation();
  const [query, setQuery] = useState('');
  const searchable = options.length > 12;
  const term = searchable ? query.trim().toLowerCase() : '';
  const matching = term ? options.filter(({ name, description }) =>
    name.toLowerCase().includes(term) || description?.toLowerCase().includes(term)) : options;
  const checked = new Set(selected);
  const update = (name: string, include: boolean) => change(include ? (checked.has(name) ? [...selected] : [...selected, name]) : selected.filter(id => id !== name));
  return <div className={css.resourceList} role="group" aria-label={label}>
    {searchable && <div className={css.filter}>
      <IconSearchOutline16 />
      <input type="search" aria-label={tx('resources:subagents.filter-choices-for', { p0: label })}
        placeholder={tx('resources:subagents.filter-choices')} value={query} onChange={event => setQuery(event.target.value)}
        onKeyDown={event => { if (event.key === 'Enter') event.preventDefault(); }} />
    </div>}
    {matching.length ? <div className={compactTools ? css.toolChoiceList : css.resourceChoices}>
      {matching.map(option => compactTools
        ? <ToolChoiceRow key={option.name} option={option} checked={checked.has(option.name)} onChange={include => update(option.name, include)} />
        : <label key={option.name} className={css.resourceChoice} title={option.description}>
          <input type="checkbox" aria-label={option.name} checked={checked.has(option.name)}
            onChange={event => update(option.name, event.target.checked)} />
          <span>
            <span className={css.choiceName}>{option.name}</span>
            {option.description && <span className={css.choiceDescription}>{option.description}</span>}
          </span>
        </label>)}
    </div> : <p className={css.noMatches} role="status">{tx('resources:subagents.no-matching-choices')}</p>}
  </div>;
}
