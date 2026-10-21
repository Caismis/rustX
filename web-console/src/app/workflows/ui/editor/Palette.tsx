import { useEffect, useRef, useState } from 'react';
import clsx from 'clsx';
import { useTranslation } from '../../../../locale/react';
import { IconSearchOutline16 } from '../../../../presentation/primitives/icons';
import { NODE_KINDS, type NodeKind } from '../../document/model';
import { KindIcon } from '../icons';
import css from './Editor.module.css';

/** The contextual, searchable Add Node palette. It lists exactly the seven
 * native node kinds and closes on choice, Escape or outside focus. */
export function Palette({ at, onChoose, onClose }: { at: { x: number; y: number }; onChoose: (kind: NodeKind) => void; onClose: () => void }) {
  const tx = useTranslation();
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const root = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  // Take the keyboard after the opening gesture has settled.
  useEffect(() => { const frame = requestAnimationFrame(() => input.current?.focus()); return () => cancelAnimationFrame(frame); }, []);
  const kinds = NODE_KINDS.filter(kind => tx(`workflows:kind.${kind}`).toLowerCase().includes(query.trim().toLowerCase()) || kind.includes(query.trim().toLowerCase()));
  useEffect(() => {
    const outside = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) onClose(); };
    document.addEventListener('pointerdown', outside, true);
    return () => document.removeEventListener('pointerdown', outside, true);
  }, [onClose]);
  return <div ref={root} className={css.palette} style={{ left: at.x, top: at.y }} role="dialog" aria-label={tx('workflows:canvas.add')}
    onKeyDown={event => {
      if (event.key === 'Escape') { event.stopPropagation(); onClose(); }
      else if (event.key === 'ArrowDown') { event.preventDefault(); setActive(index => Math.min(index + 1, kinds.length - 1)); }
      else if (event.key === 'ArrowUp') { event.preventDefault(); setActive(index => Math.max(index - 1, 0)); }
      else if (event.key === 'Enter' && kinds[active]) { event.preventDefault(); onChoose(kinds[active]); }
    }}>
    <label className={css.paletteSearch}><IconSearchOutline16 /><input ref={input} value={query} placeholder={tx('workflows:palette.search')} aria-label={tx('workflows:palette.search')}
      aria-controls="workflow-palette" aria-activedescendant={kinds[active] ? `workflow-palette-${kinds[active]}` : undefined}
      onChange={event => { setQuery(event.target.value); setActive(0); }} /></label>
    <p className={css.paletteTitle}>{tx('workflows:palette.available')}</p>
    <ul id="workflow-palette" role="listbox" aria-label={tx('workflows:palette.available')}>
      {kinds.map((kind, index) => <li key={kind} id={`workflow-palette-${kind}`} role="option" aria-selected={index === active}
        className={clsx(css.paletteOption, index === active && css.paletteActive)} onMouseEnter={() => setActive(index)} onClick={() => onChoose(kind)}>
        <KindIcon kind={kind} size={15} />{tx(`workflows:kind.${kind}`)}
      </li>)}
      {!kinds.length && <li className={css.note}>{tx('workflows:palette.none')}</li>}
    </ul>
  </div>;
}
