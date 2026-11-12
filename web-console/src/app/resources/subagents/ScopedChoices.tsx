import { useState, type ReactNode } from 'react';
import type { SourceScope } from '../../../../../protocol/app-server/v79';
import { useTranslation } from '../../../locale/react';
import { IconSearchOutline16 } from '../../../presentation/primitives/icons';
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

export function ResourceChoices({ label, options, selected, change }: {
  label: string;
  options: readonly { name: string; description?: string }[];
  selected: readonly string[];
  change: (next: string[]) => void;
}) {
  const tx = useTranslation();
  const [query, setQuery] = useState('');
  const searchable = options.length > 12;
  const term = searchable ? query.trim().toLowerCase() : '';
  const matching = term ? options.filter(({ name, description }) =>
    name.toLowerCase().includes(term) || description?.toLowerCase().includes(term)) : options;
  const checked = new Set(selected);
  return <div className={css.resourceList} role="group" aria-label={label}>
    {searchable && <div className={css.filter}>
      <IconSearchOutline16 />
      <input type="search" aria-label={tx('resources:subagents.filter-choices-for', { p0: label })}
        placeholder={tx('resources:subagents.filter-choices')} value={query} onChange={event => setQuery(event.target.value)}
        onKeyDown={event => { if (event.key === 'Enter') event.preventDefault(); }} />
    </div>}
    {matching.length ? <div className={css.resourceChoices}>
      {matching.map(({ name, description }) => <label key={name} className={css.resourceChoice} title={description}>
        <input type="checkbox" aria-label={name} checked={checked.has(name)}
          onChange={event => change(event.target.checked ? [...selected, name] : selected.filter(id => id !== name))} />
        <span>
          <span className={css.choiceName}>{name}</span>
          {description && <span className={css.choiceDescription}>{description}</span>}
        </span>
      </label>)}
    </div> : <p className={css.noMatches} role="status">{tx('resources:subagents.no-matching-choices')}</p>}
  </div>;
}
