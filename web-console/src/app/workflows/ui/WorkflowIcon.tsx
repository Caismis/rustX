import { useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import { Icon } from '@iconify/react/offline';
import { BundledIcon, hasBundledIcon, useBundledIcons } from '../../../presentation/primitives/BundledIcon';
import { useTranslation } from '../../../locale/react';
import { ICON_CATEGORIES, type IconCategory } from './iconCategories';
import { IconWorkflow } from './icons';
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
  return <div ref={root} className="flex-none">
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger ref={trigger} className={clsx(!label && 'grid place-items-center w-[46px] h-[46px] p-0 border-[0.5px] [border-color:var(--dsw-alias-border-l3)] rounded-[var(--dsw-radius-md)] bg-[var(--dsw-alias-bg-layer-1)] [color:var(--dsw-alias-label-primary)] cursor-pointer aria-expanded:[border-color:var(--dsw-alias-label-tertiary)] hover:[border-color:var(--dsw-alias-label-tertiary)] focus-visible:outline-2 focus-visible:outline-[var(--dsw-alias-state-business-primary)] focus-visible:outline-offset-1', className)} aria-label={label ?? tx('workflows:icon.choose')}
        title={label ?? tx('workflows:icon.choose')} disabled={disabled}>
        {label ?? <WorkflowIcon icon={value} size={22} />}
      </PopoverTrigger>
      <PopoverPopup
        className="z-10 flex flex-col gap-[8px] w-[400px] max-w-[calc(100vw-24px)] p-[10px] border-0 rounded-[var(--dsw-radius-md)] bg-[var(--dsw-alias-bg-layer-2)] shadow-[var(--dsw-elevation-prominent)] [&>[data-slot=popover-viewport]]:p-0 [&_input]:px-[10px] [&_input]:py-[6px] [&_input]:border-0 [&_input]:rounded-[var(--dsw-radius-sm)] [&_input]:bg-[var(--dsw-alias-bg-layer-1)] [&_input]:[color:var(--dsw-alias-label-primary)] [&_input]:[font:inherit] [&_input]:text-[13px]"
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
    <div className="flex flex-wrap gap-[4px]" role="group" aria-label={tx('workflows:icon.categories')}>
      {chips.map(id => <button key={id} type="button" aria-pressed={!search && id === category}
        className={clsx('px-[9px] py-[3px] border-[0.5px] [border-color:var(--dsw-alias-border-l3)] rounded-full [corner-shape:round] bg-transparent [color:var(--dsw-alias-label-secondary)] [font:inherit] text-[12px] cursor-pointer hover:[color:var(--dsw-alias-label-primary)] focus-visible:outline-2 focus-visible:outline-[var(--dsw-alias-state-business-primary)] focus-visible:outline-offset-1', !search && id === category && '[border-color:var(--dsw-alias-label-primary)] bg-[var(--dsw-alias-label-primary)] [color:var(--dsw-alias-bg-layer-2)] hover:[color:var(--dsw-alias-bg-layer-2)]')}
        onClick={() => { setCategory(id); setQuery(''); }}>{tx(`workflows:icon.category.${id}`)}</button>)}
    </div>
    {!set ? <p className="m-0 py-[12px] px-0 text-center text-[12px] [color:var(--dsw-alias-label-tertiary)]" role="status">{tx('workflows:icon.loading')}</p>
      : !names.length ? <p className="m-0 py-[12px] px-0 text-center text-[12px] [color:var(--dsw-alias-label-tertiary)]">{tx('workflows:icon.empty')}</p>
        : <div className="grid grid-cols-[repeat(auto-fill,minmax(34px,1fr))] gap-[4px] min-h-0 max-h-[264px] overflow-y-auto" role="listbox" aria-label={tx('workflows:icon.label')}>
          {names.map(name => {
            const icon = `${set.prefix}:${name}`;
            return <button key={name} type="button" role="option" aria-selected={icon === value} title={name} aria-label={name}
              className={clsx('grid place-items-center h-[34px] p-0 border border-transparent rounded-[var(--dsw-radius-sm)] bg-transparent [color:var(--dsw-alias-label-secondary)] cursor-pointer hover:bg-[var(--dsw-alias-bg-layer-2,var(--dsw-alias-bg-skeleton))] hover:[color:var(--dsw-alias-label-primary)] focus-visible:outline-2 focus-visible:outline-[var(--dsw-alias-state-business-primary)] focus-visible:outline-offset-1', icon === value && '[border-color:var(--dsw-alias-label-primary)] [color:var(--dsw-alias-label-primary)]')} onClick={() => onChange(icon)}>
              <Icon icon={icon} width={18} height={18} aria-hidden="true" />
            </button>;
          })}
        </div>}
  </>;
}
