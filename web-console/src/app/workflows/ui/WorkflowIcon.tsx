import { useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import { Icon } from '@iconify/react/offline';
import { BundledIcon, hasBundledIcon, useBundledIcons } from '../../../presentation/primitives/BundledIcon';
import { useTranslation } from '../../../locale/react';
import { ICON_CATEGORIES, type IconCategory } from './iconCategories';
import { IconWorkflow } from './icons';
import css from './WorkflowIcon.module.css';
import { Input as TextInput } from '@/presentation/primitives/Input';
import { Popover, PopoverPopup, PopoverTrigger } from '@/components/ui/popover';

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
  const trigger = useRef<HTMLButtonElement>(null);
  const root = useRef<HTMLDivElement>(null);
  return <div ref={root} className={css.anchor}>
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger ref={trigger} className={clsx(!label && css.tile, className)} aria-label={label ?? tx('workflows:icon.choose')}
        title={label ?? tx('workflows:icon.choose')} disabled={disabled}>
        {label ?? <WorkflowIcon icon={value} size={22} />}
      </PopoverTrigger>
      <PopoverPopup
        className={css.panel}
        role="group"
        aria-label={tx('workflows:icon.label')}
        positionMethod="fixed"
        side="bottom"
        align="start"
        sideOffset={6}
        collisionPadding={{ left: 68, right: 12, top: 12, bottom: 12 }}
        finalFocus={() => trigger.current ?? false}
        portalProps={{ container: root }}
        aria-hidden={!open}
        inert={!open}
      >
        <IconGrid value={value} onChange={icon => { onChange(icon); setOpen(false); }} />
      </PopoverPopup>
    </Popover>
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
    <TextInput type="search" value={query} autoFocus onChange={event => setQuery(event.target.value)}
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
