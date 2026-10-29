import { useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { Icon, addCollection, type IconifyJSON } from '@iconify/react/offline';
import { useTranslation } from '../../../locale/react';
import { IconWorkflow } from './icons';
import css from './WorkflowIcon.module.css';

/** The icon a Workflow shows when its display names none. */
export const DEFAULT_WORKFLOW_ICON = 'lucide:workflow';
/** Workflow-shaped starting points, shown before the author searches. */
const SUGGESTED = ['workflow', 'bot', 'sparkles', 'brain', 'file-text', 'files', 'notebook-pen', 'mail', 'message-square', 'calendar', 'clock',
  'search', 'globe', 'newspaper', 'code', 'terminal', 'bug', 'git-branch', 'database', 'table', 'chart-line', 'chart-pie', 'calculator', 'list-checks',
  'clipboard-check', 'shield-check', 'scale', 'languages', 'pen-tool', 'image', 'video', 'music', 'mic', 'book-open', 'graduation-cap', 'briefcase',
  'shopping-cart', 'wallet', 'users', 'user-round', 'heart-pulse', 'map', 'plane', 'rocket', 'zap', 'wrench', 'settings', 'package'];
const RESULTS = 120;

// The Offline build never asks the Iconify API: only bundled sets render.
let lucide: Promise<IconifyJSON> | undefined;
function loadIcons() {
  return lucide ??= import('@iconify-json/lucide/icons.json').then(module => {
    const set = module.default as IconifyJSON;
    addCollection(set);
    return set;
  });
}

function useIconSet() {
  const [set, setSet] = useState<IconifyJSON>();
  useEffect(() => {
    let live = true;
    void loadIcons().then(value => { if (live) setSet(value); });
    return () => { live = false; };
  }, []);
  return set;
}

function known(set: IconifyJSON | undefined, icon: string) {
  const [prefix, name] = icon.split(':');
  return !!set && prefix === set.prefix && !!name && (name in set.icons || name in (set.aliases ?? {}));
}

/** A Workflow's chosen icon; the Workflow glyph until a bundled set has it. */
export function WorkflowIcon({ icon, size = 18, className }: { icon?: string | null; size?: number; className?: string }) {
  const set = useIconSet();
  return known(set, icon ?? DEFAULT_WORKFLOW_ICON)
    ? <Icon icon={icon ?? DEFAULT_WORKFLOW_ICON} width={size} height={size} className={className} aria-hidden="true" />
    : <IconWorkflow size={size} className={className} />;
}

/** An inline, searchable grid over the bundled icon set. */
export function IconPicker({ value, onChange, disabled }: { value: string; onChange: (icon: string) => void; disabled?: boolean }) {
  const tx = useTranslation();
  const set = useIconSet();
  const [query, setQuery] = useState('');
  const names = useMemo(() => {
    if (!set) return [];
    const search = query.trim().toLowerCase().replace(/\s+/g, '-');
    // The current choice always leads, so it stays visible as selected.
    const current = value.startsWith(`${set.prefix}:`) && known(set, value) ? [value.slice(set.prefix.length + 1)] : [];
    if (!search) return [...new Set([...current, ...SUGGESTED])].filter(name => known(set, `${set.prefix}:${name}`));
    return Object.keys(set.icons).filter(name => name.includes(search)).slice(0, RESULTS);
  }, [set, query, value]);
  return <div className={css.picker}>
    <input type="search" value={query} onChange={event => setQuery(event.target.value)} disabled={disabled}
      placeholder={tx('workflows:icon.search')} aria-label={tx('workflows:icon.search')} />
    {!set ? <p className={css.state} role="status">{tx('workflows:icon.loading')}</p>
      : !names.length ? <p className={css.state}>{tx('workflows:icon.empty')}</p>
        : <div className={css.grid} role="listbox" aria-label={tx('workflows:icon.label')}>
          {names.map(name => {
            const icon = `${set.prefix}:${name}`;
            return <button key={name} type="button" role="option" aria-selected={icon === value} title={name} aria-label={name} disabled={disabled}
              className={clsx(css.option, icon === value && css.selected)} onClick={() => onChange(icon)}>
              <Icon icon={icon} width={18} height={18} aria-hidden="true" />
            </button>;
          })}
        </div>}
  </div>;
}
