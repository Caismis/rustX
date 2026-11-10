import { useEffect, useMemo, useRef, useState } from 'react';
import { autoUpdate, flip, offset, shift, size, useFloating } from '@floating-ui/react-dom';
import clsx from 'clsx';
import { Icon } from '@iconify/react/offline';
import { BundledIcon, hasBundledIcon, useBundledIcons } from '../../../presentation/primitives/BundledIcon';
import { useTranslation } from '../../../locale/react';
import { ICON_CATEGORIES, type IconCategory } from './iconCategories';
import { IconWorkflow } from './icons';
import css from './WorkflowIcon.module.css';

/** The icon a Workflow shows when its display names none. */
export const DEFAULT_WORKFLOW_ICON = 'lucide:workflow';

/** A Workflow's chosen icon; the Workflow glyph until a bundled set has it. */
export function WorkflowIcon({ icon, size = 18, className }: { icon?: string | null; size?: number; className?: string }) {
  return <BundledIcon name={icon ?? DEFAULT_WORKFLOW_ICON} size={size} className={className}
    fallback={<IconWorkflow size={size} className={className} />} />;
}

/** The chosen icon as a tile; clicking it floats a searchable grid over the
 * bundled set beside it. Choosing, Escape or a press outside closes it. */
export function IconPicker({ value, onChange, disabled, className, label }: {
  value: string; onChange: (icon: string) => void; disabled?: boolean; className?: string; label?: string;
}) {
  const tx = useTranslation();
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const { refs, floatingStyles } = useFloating({
    open,
    placement: 'bottom-start',
    strategy: 'fixed',
    whileElementsMounted: autoUpdate,
    middleware: [offset(6), flip({ padding: 12 }), shift({ padding: 12 }),
      size({ padding: 12, apply: ({ availableHeight, elements }) => { elements.floating.style.maxHeight = `${Math.max(160, availableHeight)}px`; } })],
  });
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false); };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, [open]);
  return <div ref={root} className={css.anchor}
    // Escape closes the panel first; the enclosing dialog skips a handled key.
    onKeyDown={event => { if (open && event.key === 'Escape') { event.preventDefault(); setOpen(false); } }}>
    <button ref={refs.setReference} type="button" className={clsx(!label && css.tile, className)} aria-haspopup="listbox" aria-expanded={open}
      aria-label={label ?? tx('workflows:icon.choose')} title={label ?? tx('workflows:icon.choose')} disabled={disabled} onClick={() => setOpen(value => !value)}>
      {label ?? <WorkflowIcon icon={value} size={22} />}
    </button>
    {open && <div ref={refs.setFloating} style={floatingStyles} className={css.panel}>
      <IconGrid value={value} onChange={icon => { onChange(icon); setOpen(false); }} />
    </div>}
  </div>;
}

function IconGrid({ value, onChange }: { value: string; onChange: (icon: string) => void }) {
  const tx = useTranslation();
  const set = useBundledIcons();
  const [query, setQuery] = useState('');
  const [category, setCategory] = useState<IconCategory | 'all'>('common');
  const search = query.trim().toLowerCase().replace(/\s+/g, '-');
  const names = useMemo(() => {
    if (!set) return [];
    const all = Object.keys(set.icons);
    // A search spans the whole set; otherwise the chosen category, with the
    // current choice leading the common one so it stays visible as selected.
    if (search) return all.filter(name => name.includes(search));
    if (category === 'all') return all;
    const current = category === 'common' && value.startsWith(`${set.prefix}:`) ? [value.slice(set.prefix.length + 1)] : [];
    return [...new Set([...current, ...ICON_CATEGORIES[category]])].filter(name => hasBundledIcon(set, `${set.prefix}:${name}`));
  }, [set, search, category, value]);
  const chips: (IconCategory | 'all')[] = ['all', ...Object.keys(ICON_CATEGORIES) as IconCategory[]];
  return <>
    <input type="search" value={query} autoFocus onChange={event => setQuery(event.target.value)}
      placeholder={tx('workflows:icon.search')} aria-label={tx('workflows:icon.search')} />
    <div className={css.categories} role="group" aria-label={tx('workflows:icon.categories')}>
      {chips.map(id => <button key={id} type="button" aria-pressed={!search && id === category}
        className={clsx(css.category, !search && id === category && css.categoryActive)}
        onClick={() => { setCategory(id); setQuery(''); }}>{tx(`workflows:icon.category.${id}`)}</button>)}
    </div>
    {!set ? <p className={css.state} role="status">{tx('workflows:icon.loading')}</p>
      : !names.length ? <p className={css.state}>{tx('workflows:icon.empty')}</p>
        : <div className={css.grid} role="listbox" aria-label={tx('workflows:icon.label')}>
          {names.map(name => {
            const icon = `${set.prefix}:${name}`;
            return <button key={name} type="button" role="option" aria-selected={icon === value} title={name} aria-label={name}
              className={clsx(css.option, icon === value && css.selected)} onClick={() => onChange(icon)}>
              <Icon icon={icon} width={18} height={18} aria-hidden="true" />
            </button>;
          })}
        </div>}
  </>;
}
