import { Fragment, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { modelDefaultLabel, offersModelDefault, profileUnavailable, type ModelChoice, type ModelSelectionIntent } from '../../presentation/agent/ModelSelect';
import { selectionChanges } from '../../bindings/model-catalog';
import { useTranslation } from '../../locale/react';
import menuCss from '../commands/Commands.module.css';
import css from './ModelPicker.module.css';

/** `profile` is the configured (pinned) Profile, absent while following the
 * Model default; `effectiveProfile` is only what the invocation resolves to. */
export interface ModelPickerState {
  choices: ModelChoice[]; current?: string; profile?: string; effectiveProfile?: string; disabled: boolean; loading: boolean; error?: string;
  choose: (intent: ModelSelectionIntent) => Promise<boolean>;
}
/** The composer and slash picker share the same attachment-scoped catalog.
 * Each row is one unambiguous gesture: a Model row never clears a pinned
 * Profile; a Model's default row is the explicit return to following its
 * default, offered too for clearing a pinned Profile the Model no longer
 * declares. */
export function ModelPicker({ state, close, chosen }: { state: ModelPickerState; close: () => void; chosen: () => void }) {
  const tx = useTranslation();
  const [query, setQuery] = useState('');
  // The configured selection's own row: its pinned Profile, else its default-
  // Profile row, else the Model row — also for an unavailable pinned Profile,
  // which has no row.
  const selected = state.choices.find(choice => choice.id === state.current);
  const stale = selected && profileUnavailable(selected, state.profile);
  const [active, setActive] = useState<string | undefined>(() => JSON.stringify(state.profile !== undefined && !stale ? [state.current, state.profile]
    : state.profile === undefined && selected?.profiles.length ? [state.current, null] : [state.current]));
  const [pending, setPending] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const rows = state.choices.flatMap(choice => [
    { key: JSON.stringify([choice.id]), intent: { kind: 'model', model: choice.id } as ModelSelectionIntent, label: choice.id, suffix: undefined as string | undefined },
    ...(offersModelDefault(choice, state.current, state.profile) ? [{ key: JSON.stringify([choice.id, null]), intent: { kind: 'model-default', model: choice.id } as ModelSelectionIntent, label: `${choice.id} / ${modelDefaultLabel(tx, choice)}`, suffix: modelDefaultLabel(tx, choice) }] : []),
    ...choice.profiles.map(profile => ({ key: JSON.stringify([choice.id, profile.id]), intent: { kind: 'profile', model: choice.id, profile: profile.id } as ModelSelectionIntent, label: `${choice.id} / ${profile.id}`, suffix: profile.id })),
  ]).filter(row => row.label.toLowerCase().includes(query.trim().toLowerCase()));
  const index = Math.max(0, rows.findIndex(row => row.key === active));
  const select = async (row: typeof rows[number]) => {
    if (state.disabled || pending) return;
    if (!selectionChanges(state.current, state.profile, row.intent)) { chosen(); return; }
    setPending(true);
    try { if (await state.choose(row.intent)) chosen(); } finally { setPending(false); }
  };
  useEffect(() => {
    const outside = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) close(); };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, [close]);
  useLayoutEffect(() => {
    const row = root.current?.querySelector<HTMLElement>('[aria-selected="true"]');
    const list = row?.parentElement;
    if (!row || !list) return;
    if (row.offsetTop < list.scrollTop) list.scrollTop = row.offsetTop;
    else if (row.offsetTop + row.offsetHeight > list.scrollTop + list.clientHeight) list.scrollTop = row.offsetTop + row.offsetHeight - list.clientHeight;
  }, [active, query]);
  const provider = (id: string) => id.includes('/') ? id.slice(0, id.indexOf('/')) : '';
  return <div ref={root} className={`${menuCss.menu} ${css.root}`} data-model-picker onKeyDown={event => {
    if (event.nativeEvent.isComposing) return;
    if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(); }
    if (rows.length && (event.key === 'ArrowDown' || event.key === 'ArrowUp')) { event.preventDefault(); setActive(rows[(index + (event.key === 'ArrowDown' ? 1 : rows.length - 1)) % rows.length].key); }
    if (event.key === 'Enter') { event.preventDefault(); if (rows[index]) void select(rows[index]); }
  }}>
    <input autoFocus className={css.search} aria-label={tx('agent:model-picker.search')} placeholder={tx('agent:model-picker.search')} value={query} onChange={event => { setQuery(event.target.value); setActive(undefined); }} role="combobox" aria-expanded="true" aria-controls="composer-models" aria-activedescendant={rows[index] ? `composer-model-${index}` : undefined}/>
    <div id="composer-models" role="listbox" aria-label={tx('agent:model-select.model')} className={css.options}>
      {rows.map((row, i) => <Fragment key={row.key}>
        {(i === 0 || provider(row.intent.model) !== provider(rows[i - 1].intent.model)) && provider(row.intent.model) && <div className={menuCss.sectionTitle} role="presentation">{provider(row.intent.model)}</div>}
        <button type="button" tabIndex={-1} id={`composer-model-${i}`} role="option" aria-label={row.label} aria-selected={i === index} disabled={state.disabled || pending} className={menuCss.menuRow}
          onMouseMove={() => setActive(row.key)} onMouseDown={event => event.preventDefault()} onClick={() => void select(row)}>{row.intent.model.includes('/') ? row.intent.model.slice(row.intent.model.indexOf('/') + 1) : row.intent.model}{row.suffix !== undefined && ` / ${row.suffix}`}</button>
      </Fragment>)}
      {!rows.length && <p role="status">{tx(state.loading ? 'agent:model-select.reading-native-models' : 'agent:model-picker.empty')}</p>}
    </div>
    {!state.loading && stale && <p role="status">{tx('agent:model-select.profile')}{' '}{state.profile} {tx('agent:model-select.is-unavailable-for')}{' '}{state.current} {tx('agent:model-select.in-this-workspace')}</p>}
    {state.error && <p role="alert">{state.error}</p>}
  </div>;
}
