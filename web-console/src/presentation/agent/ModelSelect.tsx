/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-model-selection/ModelSelect; see PROVENANCE.md. */
import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent } from 'react';
import { createPortal } from 'react-dom';
import clsx from 'clsx';
import { useTranslation } from '../../locale/react';
import { useDialogPortal } from '../primitives/DialogSurface';
import { MenuSurface } from '../primitives/MenuSurface';
import { MenuGroup, observeStickyMenuGroups } from '../primitives/MenuGroup';
import { rankByName } from '../primitives/rank-by-name';
import { StateDot } from '../primitives/StateDot';
import { IconCheckOutline16, IconCloseFill14, IconDataOutline16, IconChevronDownOutline14, IconChevronRightOutline14 } from '../primitives/icons';
import css from './ModelSelect.module.css';

export interface ModelChoice { id: string; profiles: { id: string; label: string }[]; defaultProfile?: string }
/** Exact native configuration intent. Choosing the current Model keeps its
 * pinned Profile; following its default clears only that pin. */
export type ModelSelectionIntent =
 | { kind: 'model'; model: string }
 | { kind: 'profile'; model: string; profile: string }
 | { kind: 'model-default'; model: string };
type Translate = ReturnType<typeof useTranslation>;
export function profileUnavailable(choice: ModelChoice, profile?: string) {
 return profile !== undefined && !choice.profiles.some(item => item.id === profile);
}
export function offersModelDefault(choice: ModelChoice, current?: string, profile?: string) {
 return choice.profiles.length > 0 || choice.id === current && profileUnavailable(choice, profile);
}
export function modelDefaultLabel(tx: Translate, choice: ModelChoice) {
 return choice.profiles.find(item => item.id === choice.defaultProfile)?.label
   ?? tx(choice.profiles.length ? 'agent:model-select.provider-default' : 'agent:model-select.use-model-defaults');
}
/** A declared default occupies its own named effort row, never an additional
 * duplicate "Model default profile" row. Native defaults without a named
 * Profile, and stale-pin recovery, retain one explicit default action. */
export function modelEffortChoices(tx: Translate, choice: ModelChoice, current?: string, profile?: string) {
 const declaredDefault = choice.profiles.some(item => item.id === choice.defaultProfile);
 return [
  ...(!declaredDefault && offersModelDefault(choice, current, profile)
   ? [{ key: 'default', label: modelDefaultLabel(tx, choice), profile: undefined, intent: { kind: 'model-default', model: choice.id } as ModelSelectionIntent }] : []),
  ...choice.profiles.map(item => ({ key: `profile:${item.id}`, label: item.label, profile: item.id,
   intent: item.id === choice.defaultProfile ? { kind: 'model-default', model: choice.id } as ModelSelectionIntent
    : { kind: 'profile', model: choice.id, profile: item.id } as ModelSelectionIntent })),
 ];
}
type Pane = 'root' | 'model' | 'effort';
const MEASURE_STYLE: CSSProperties = { visibility: 'hidden', left: 0, top: 0 };

/** DSH's single-card Model / Thinking effort panes over rustX's native catalog.
 * Pending selections keep their pane, rows and confirmed trigger label until
 * native acknowledgement and the authoritative reread have both completed. */
export function ModelSelect({ choices, current, profile, effectiveProfile, disabled, loading, error, load, choose, nextPage, initialOpen = false, binding = '' }: {
 choices: ModelChoice[]; current?: string; profile?: string; effectiveProfile?: string; disabled: boolean; loading: boolean; error?: string;
 load: (refresh?: boolean) => void; choose: (intent: ModelSelectionIntent) => Promise<boolean>; nextPage?: () => Promise<void>; initialOpen?: boolean; binding?: string;
}) {
 const tx = useTranslation(), id = useId(), portal = useDialogPortal();
 const [open, setOpen] = useState(initialOpen), [pane, setPane] = useState<Pane>('root');
 const [query, setQuery] = useState(''), [highlight, setHighlight] = useState<number | null>(null);
 const [selectionFocus, setSelectionFocus] = useState(false);
 const [pending, setPending] = useState<ModelSelectionIntent>();
 const [owner, setOwner] = useState(binding);
 const ownership = useRef({ binding });
 if (owner !== binding) {
  ownership.current = { binding }; setOwner(binding); setOpen(false); setPane('root'); setPending(undefined); setQuery(''); setHighlight(null);
 }
 const root = useRef<HTMLDivElement>(null), trigger = useRef<HTMLButtonElement>(null), menu = useRef<HTMLDivElement>(null);
 const groups = useRef<HTMLDivElement>(null), search = useRef<HTMLInputElement>(null);
 const [position, setPosition] = useState<CSSProperties | null>(null);
 const lastAction = useRef<'load' | 'select'>('load');
 const focusPane = useRef<'drill' | 'model' | 'effort' | undefined>(undefined);
 const inFlight = useRef<{ owner: object } | undefined>(undefined);
 const selected = choices.find(choice => choice.id === current);
 const displayed = effectiveProfile ?? profile ?? selected?.defaultProfile;
 const effortLabel = selected?.profiles.find(item => item.id === displayed)?.label ?? displayed ?? (selected?.profiles.length ? modelDefaultLabel(tx, selected) : undefined);
 const efforts = selected ? modelEffortChoices(tx, selected, current, profile) : [];
 const busy = pending !== undefined;
 const showSearch = choices.length > 4;
 const previousSearch = useRef(showSearch);
 const modelGroups = useMemo(() => {
  const grouped = new Map<string, (ModelChoice & { name: string })[]>();
  for (const choice of choices) {
   const provider = choice.id.includes('/') ? choice.id.slice(0, choice.id.indexOf('/')) : '';
   const group = grouped.get(provider) ?? []; group.push({ ...choice, name: choice.id }); grouped.set(provider, group);
  }
  return [...grouped].map(([provider, models]) => ({ provider, models: rankByName(models, showSearch ? query.trim() : '') })).filter(group => group.models.length);
 }, [choices, query, showSearch]);
 const visibleModels = modelGroups.flatMap(group => group.models);
 const checkedIndex = visibleModels.findIndex(choice => choice.id === current);
 const activeIndex = Math.min(highlight ?? Math.max(0, checkedIndex), visibleModels.length - 1);
 const close = (restore = false) => {
  setOpen(false); setPane('root');
  if (restore) trigger.current?.focus();
 };
 const drill = (next: Pane) => { focusPane.current = 'drill'; setQuery(''); setHighlight(null); setPane(next); };
 const back = () => { if (pane !== 'root') focusPane.current = pane; setPane('root'); };
 const submit = async (intent: ModelSelectionIntent) => {
  if (disabled || loading || busy || inFlight.current?.owner === ownership.current) return;
  const at = ownership.current, request = { owner: at }; inFlight.current = request;
  lastAction.current = 'select'; setPending(intent); setSelectionFocus(true); trigger.current?.focus();
  try {
   if (await choose(intent) && ownership.current === at) close(true);
  } finally {
   if (inFlight.current === request) inFlight.current = undefined;
   if (ownership.current === at) setPending(undefined);
  }
 };
 useEffect(() => {
  if (!open) return;
  const outside = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node) && !menu.current?.contains(event.target as Node)) close(); };
  document.addEventListener('pointerdown', outside);
  return () => document.removeEventListener('pointerdown', outside);
 }, [open]);
 useLayoutEffect(() => {
  if (!open) { setPosition(null); return; }
  const place = () => {
   const rect = trigger.current?.getBoundingClientRect(), card = menu.current;
   if (!rect || !card) return;
   const left = Math.max(12, Math.min(rect.right - card.offsetWidth, innerWidth - card.offsetWidth - 12));
   const top = Math.max(12, Math.min(rect.top - 8 - card.offsetHeight, innerHeight - card.offsetHeight - 12));
   setPosition(previous => previous?.left === left && previous?.top === top ? previous : { left, top });
  };
  place(); window.addEventListener('scroll', place, true); window.addEventListener('resize', place);
  const observer = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(place);
  if (menu.current) observer?.observe(menu.current);
  return () => { observer?.disconnect(); window.removeEventListener('scroll', place, true); window.removeEventListener('resize', place); };
 }, [open, pane, choices, loading, error, query]);
 useLayoutEffect(() => {
  const intent = focusPane.current ?? (previousSearch.current !== showSearch && pane === 'model' ? 'drill' : undefined);
  if (!open || !position || !intent) return;
  previousSearch.current = showSearch; focusPane.current = undefined;
  const target = intent !== 'drill' ? menu.current?.querySelector<HTMLElement>(`[data-pane="${intent}"]`)
   : pane === 'model' && showSearch ? search.current
   : menu.current?.querySelector<HTMLElement>('[aria-checked="true"]:not(:disabled)') ?? menu.current?.querySelector<HTMLElement>('[role="menuitemradio"]:not(:disabled)');
  (target ?? trigger.current)?.focus();
 }, [open, pane, showSearch, position !== null]);
 useEffect(() => { if (groups.current) return observeStickyMenuGroups(groups.current); }, [open, pane, modelGroups]);
 useLayoutEffect(() => {
  if (open && pane === 'model') document.getElementById(`${id}-model-${activeIndex}`)?.scrollIntoView?.({ block: 'nearest' });
 }, [open, pane, id, activeIndex]);
 const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
  if (!open || event.nativeEvent.isComposing) return;
  if (event.key === 'Escape' || event.key === 'Tab' && event.shiftKey || event.key === 'ArrowLeft' && pane !== 'root') {
   event.preventDefault(); event.stopPropagation();
   if (pane !== 'root' && selected) back(); else close(true);
   return;
  }
  if (pane === 'model' && showSearch && (event.key === 'ArrowDown' || event.key === 'ArrowUp')) {
   event.preventDefault(); if (!busy && visibleModels.length) { setHighlight((activeIndex + (event.key === 'ArrowDown' ? 1 : visibleModels.length - 1)) % visibleModels.length); search.current?.focus(); } return;
  }
  if (pane === 'model' && showSearch && event.target === search.current && (event.key === 'Enter' || event.key === 'Tab')) {
   if (event.key === 'Tab' && !visibleModels.length) return;
   event.preventDefault(); if (visibleModels[activeIndex]) void submit({ kind: 'model', model: visibleModels[activeIndex].id }); return;
  }
  const rows = [...menu.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled), [role="menuitemradio"]:not(:disabled)') ?? []];
  const focused = document.activeElement as HTMLButtonElement, index = rows.indexOf(focused);
  if (event.key === 'Tab' || event.key === 'ArrowRight' && pane === 'root') {
   event.preventDefault(); if (index >= 0) focused.click(); else (rows.find(row => row.getAttribute('aria-checked') === 'true') ?? rows[0])?.focus();
  } else if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
   event.preventDefault();
   const next = event.key === 'Home' ? 0 : event.key === 'End' ? rows.length - 1 : index < 0 ? event.key === 'ArrowDown' ? 0 : rows.length - 1 : (index + (event.key === 'ArrowDown' ? 1 : rows.length - 1)) % rows.length;
   rows[next]?.focus();
  }
 };
 const effortChecked = (row: typeof efforts[number]) => profile === undefined ? row.intent.kind === 'model-default' : row.profile === profile;
 const pendingRow = (intent: ModelSelectionIntent) => pending?.model === intent.model && (pending.kind === 'model' ? intent.kind === 'model' : pending.kind === intent.kind && (intent.kind !== 'profile' || pending.kind === 'profile' && pending.profile === intent.profile));
 let modelIndex = 0;
 const status = <>
  {loading && !busy && !choices.length && <div className={css.status} role="status">{tx('agent:model-select.reading-native-models')}</div>}
  {error && lastAction.current === 'load' && <div className={css.error}><span role="alert">{error}</span><button type="button" className={css.retry} disabled={disabled || busy} onClick={() => { lastAction.current = 'load'; load(true); }}>{tx('agent:agent-controls.reread-models')}</button></div>}
  {current && !loading && !selected && <div className={css.warning} role="status">{current} {tx('agent:model-select.is-unavailable-in-this-workspace')}</div>}
  {selected && !loading && profileUnavailable(selected, profile) && <div className={css.warning} role="status">{tx('agent:model-select.effort')} {profile} {tx('agent:model-select.is-unavailable-for')} {current} {tx('agent:model-select.in-this-workspace')}</div>}
 </>;
 return <div ref={root} className={css.root} onKeyDown={onKeyDown}
  onBlur={event => { if (!(event.relatedTarget instanceof Node && (root.current?.contains(event.relatedTarget) || menu.current?.contains(event.relatedTarget)))) close(); }}
  onMouseDown={event => { if (event.target instanceof Element && event.target.closest('button')) event.preventDefault(); }}>
  <button ref={trigger} data-model-select="" type="button" className={css.trigger} aria-label={tx('agent:model-select.model-and-effort')} title={[current ?? tx('agent:model-select.choose-model'), effortLabel].filter(Boolean).join(' · ')}
   aria-haspopup="menu" aria-expanded={open} aria-controls={open ? `${id}-menu` : undefined} aria-busy={busy} disabled={disabled && !busy} data-selection-focus={selectionFocus ? '' : undefined}
   onBlur={() => setSelectionFocus(false)} onClick={() => {
    if (open) close(true); else { setSelectionFocus(false); trigger.current?.focus(); setQuery(''); setHighlight(null); if (!selected) focusPane.current = 'drill'; setPane(selected ? 'root' : 'model'); setOpen(true); lastAction.current = 'load'; load(); }
   }}>
   <IconDataOutline16 size={16} className={css.triggerIcon}/><span className={css.triggerLabel}>{current ?? tx('agent:model-select.choose-model')}</span>
   {effortLabel && <span className={css.triggerEffort}>{effortLabel}</span>}
   <span className={css.triggerIndicator}>{busy ? <StateDot state="ongoing"/> : <IconChevronDownOutline14 className={clsx(css.chevron, open && css.chevronOpen)}/>}</span>
  </button>
  {open && createPortal(<MenuSurface ref={menu} id={`${id}-menu`} data-model-menu="" data-pane={pane} className={css.menu} style={position ?? MEASURE_STYLE} role="menu" aria-label={tx('agent:model-select.model-and-effort')} aria-busy={loading || busy}>
   {pane === 'root' && <>
    <button type="button" role="menuitem" aria-label={tx('agent:model-select.model')} data-pane="model" className={css.cell} onClick={() => drill('model')}><span className={css.cellLabel}>{tx('agent:model-select.model')}</span><span className={css.cellValue}>{current}</span><IconChevronRightOutline14 className={css.cellChevron}/></button>
    {!!efforts.length && <button type="button" role="menuitem" aria-label={tx('agent:model-select.effort')} data-pane="effort" className={css.cell} onClick={() => drill('effort')}><span className={css.cellLabel}>{tx('agent:model-select.effort')}</span><span className={css.cellValue}>{effortLabel ?? modelDefaultLabel(tx, selected!)}</span><IconChevronRightOutline14 className={css.cellChevron}/></button>}
   </>}
   {pane === 'model' && <>
    {showSearch && <div className={css.searchRow}><div className={clsx(css.search, query && css.searchWithQuery)}><input ref={search} role="searchbox" type="text" aria-label={tx('agent:model-picker.search')} placeholder={tx('agent:model-picker.search')} aria-controls={`${id}-models`} aria-activedescendant={activeIndex < 0 ? undefined : `${id}-model-${activeIndex}`} value={query} readOnly={busy} onChange={event => { setQuery(event.target.value); setHighlight(0); }}/></div>{query && <button type="button" className={css.searchClear} aria-label={tx('agent:model-picker.clear-search')} disabled={busy} onClick={() => { setQuery(''); setHighlight(0); search.current?.focus(); }}><IconCloseFill14/></button>}</div>}
    <div ref={groups} id={`${id}-models`} className={css.groups}>
     {modelGroups.map(group => <MenuGroup key={group.provider} label={group.provider}>{group.models.map(choice => {
      const index = modelIndex++, intent: ModelSelectionIntent = { kind: 'model', model: choice.id };
      return <button type="button" role="menuitemradio" aria-checked={choice.id === current} key={choice.id} aria-label={choice.id} id={`${id}-model-${index}`} tabIndex={showSearch ? -1 : 0} data-highlighted={index === activeIndex ? '' : undefined}
       className={clsx(css.option, css.modelOption, index === activeIndex && css.optionActive)} disabled={disabled || loading || busy} title={choice.id}
       onFocus={() => setHighlight(index)} onMouseMove={() => { if (!busy) { if (showSearch) setHighlight(index); else document.getElementById(`${id}-model-${index}`)?.focus(); } }} onClick={() => void submit(intent)}>
       <span className={css.optionCopy}><span className={css.modelName}>{choice.id.includes('/') ? choice.id.slice(choice.id.indexOf('/') + 1) : choice.id}</span></span><span className={css.check}>{pendingRow(intent) ? <StateDot state="ongoing"/> : choice.id === current && <IconCheckOutline16/>}</span>
      </button>;
     })}</MenuGroup>)}
    </div>
    {!loading && !visibleModels.length && <div className={css.empty} role="status">{tx('agent:model-picker.empty')}</div>}
    {nextPage && <button type="button" className={css.option} disabled={loading || busy} onClick={() => void nextPage()}>{tx('common:activity.show-more')}</button>}
   </>}
   {pane === 'effort' && efforts.map(row => <button type="button" role="menuitemradio" aria-checked={effortChecked(row)} key={row.key} className={css.option} disabled={disabled || loading || busy} onClick={() => void submit(row.intent)}><span className={css.optionCopy}><span className={css.modelName}>{row.label}</span></span><span className={css.check}>{pendingRow(row.intent) ? <StateDot state="ongoing"/> : effortChecked(row) && <IconCheckOutline16/>}</span></button>)}
   {status}
   {error && lastAction.current === 'select' && !busy && <button type="button" className={css.option} disabled={disabled} onClick={() => { lastAction.current = 'load'; load(true); }}>{tx('agent:agent-controls.reread-models')}</button>}
  </MenuSurface>, portal)}
 </div>;
}
