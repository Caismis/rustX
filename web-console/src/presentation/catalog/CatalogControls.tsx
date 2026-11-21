import { IconSearchOutline16 } from '../primitives/icons';
import css from './Catalog.module.css';

export function CatalogSearch({ label, value, onChange }: { label: string; value: string; onChange: (value: string) => void }) {
  return <label className={css.search}><IconSearchOutline16 /><input type="search" aria-label={label} placeholder={label} value={value} onChange={event => onChange(event.target.value)} /></label>;
}
