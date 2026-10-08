import { Fragment, useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { ModelChoice } from '../../presentation/agent/ModelSelect';
import { useTranslation } from '../../locale/react';
import menuCss from '../commands/Commands.module.css';
import css from './ModelPicker.module.css';

export interface ModelPickerState {
  choices: ModelChoice[]; current?: string; disabled: boolean; loading: boolean; error?: string;
  choose: (model: string) => Promise<boolean>;
}
/** The composer and slash picker share the same attachment-scoped catalog. */
export function ModelPicker({ state, close, chosen }: { state: ModelPickerState; close: () => void; chosen: () => void }) {
  const tx = useTranslation();
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(state.current);
  const [pending, setPending] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const rows = state.choices.filter(row => row.id.toLowerCase().includes(query.trim().toLowerCase()));
  const index = Math.max(0, rows.findIndex(row => row.id === active));
  const select = async (id: string) => {
    if (state.disabled || pending) return;
    if (id === state.current) { chosen(); return; }
    setPending(true);
    try { if (await state.choose(id)) chosen(); } finally { setPending(false); }
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
    if (rows.length && (event.key === 'ArrowDown' || event.key === 'ArrowUp')) { event.preventDefault(); setActive(rows[(index + (event.key === 'ArrowDown' ? 1 : rows.length - 1)) % rows.length].id); }
    if (event.key === 'Enter') { event.preventDefault(); if (rows[index]) void select(rows[index].id); }
  }}>
    <input autoFocus className={css.search} aria-label={tx('agent:model-picker.search')} placeholder={tx('agent:model-picker.search')} value={query} onChange={event => { setQuery(event.target.value); setActive(undefined); }} role="combobox" aria-expanded="true" aria-controls="composer-models" aria-activedescendant={rows[index] ? `composer-model-${index}` : undefined}/>
    <div id="composer-models" role="listbox" aria-label={tx('agent:model-select.model')} className={css.options}>
      {rows.map((row, i) => <Fragment key={row.id}>
        {(i === 0 || provider(row.id) !== provider(rows[i - 1].id)) && provider(row.id) && <div className={menuCss.sectionTitle} role="presentation">{provider(row.id)}</div>}
        <button type="button" tabIndex={-1} id={`composer-model-${i}`} role="option" aria-label={row.id} aria-selected={i === index} disabled={state.disabled || pending} className={menuCss.menuRow}
          onMouseMove={() => setActive(row.id)} onMouseDown={event => event.preventDefault()} onClick={() => void select(row.id)}>{row.id.includes('/') ? row.id.slice(row.id.indexOf('/') + 1) : row.id}</button>
      </Fragment>)}
      {!rows.length && <p role="status">{tx(state.loading ? 'agent:model-select.reading-native-models' : 'agent:model-picker.empty')}</p>}
    </div>
    {state.error && <p role="alert">{state.error}</p>}
  </div>;
}
