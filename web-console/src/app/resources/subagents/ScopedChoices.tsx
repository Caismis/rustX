import type { ReactNode } from 'react';
import type { SourceScope } from '../../../../../protocol/app-server/v77';
import { useTranslation } from '../../../locale/react';
import css from './Subagents.module.css';

export function resourceScopes(scope: SourceScope): readonly SourceScope[] {
  return scope === 'workspace' ? ['workspace', 'user'] : ['user'];
}

export function ScopedChoices({ owner, children }: { owner: SourceScope; children: ReactNode }) {
  const tx = useTranslation();
  const label = tx(owner === 'workspace' ? 'resources:scope.workspace' : 'resources:scope.global');
  return <section aria-label={label} className={css.scopedChoices}>
    <h4>{label}</h4>
    {children}
  </section>;
}

export function ResourceChoices({ label, options, selected, change }: {
  label: string;
  options: readonly { name: string; description?: string }[];
  selected: readonly string[];
  change: (next: string[]) => void;
}) {
  const checked = new Set(selected);
  return <div className={css.resourceChoices} role="group" aria-label={label}>
    {options.map(({ name, description }) => <label key={name} className={css.resourceChoice} title={description}>
      <input type="checkbox" aria-label={name} checked={checked.has(name)}
        onChange={event => change(event.target.checked ? [...selected, name] : selected.filter(id => id !== name))} />
      <span>
        <span className={css.choiceName}>{name}</span>
        {description && <span className={css.choiceDescription}>{description}</span>}
      </span>
    </label>)}
  </div>;
}
