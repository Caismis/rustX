import clsx from 'clsx';
import { useTranslation } from '../../locale/react';
import { IconSearchOutline16 } from '../primitives/icons';
import css from './Catalog.module.css';

export type CatalogView = 'cards' | 'list';

export function CatalogSearch({ label, value, onChange }: { label: string; value: string; onChange: (value: string) => void }) {
  return <label className={css.search}><IconSearchOutline16 /><input type="search" aria-label={label} placeholder={label} value={value} onChange={event => onChange(event.target.value)} /></label>;
}

export function CatalogViewSwitch({ view, onChange }: { view: CatalogView; onChange: (view: CatalogView) => void }) {
  const tx = useTranslation();
  return <div className={css.viewSwitch} role="group" aria-label={tx('workflows:library.view')}>
    {(['cards', 'list'] as const).map(option => <button key={option} type="button" aria-pressed={view === option}
      aria-label={tx(option === 'cards' ? 'workflows:library.view-cards' : 'workflows:library.view-list')}
      className={clsx(view === option && css.viewActive)} onClick={() => onChange(option)}>
      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        {option === 'cards' ? <><rect x="3" y="3" width="7" height="7" rx="1.5" /><rect x="14" y="3" width="7" height="7" rx="1.5" /><rect x="3" y="14" width="7" height="7" rx="1.5" /><rect x="14" y="14" width="7" height="7" rx="1.5" /></>
          : <path d="M4 6h2M10 6h10M4 12h2M10 12h10M4 18h2M10 18h10" />}
      </svg>
    </button>)}
  </div>;
}
