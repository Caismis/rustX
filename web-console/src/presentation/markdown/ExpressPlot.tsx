import { useId, useState, type CSSProperties, type KeyboardEvent, type PointerEvent, type ReactNode } from 'react';
import css from './ExpressPlot.module.css';

export interface PlotDetails {
  title: string;
  rows: readonly { label: string; value: string; color?: string; emphasis?: boolean }[];
}

export function ExpressPlot({ label, positions, details, children }: {
  label?: string;
  positions: readonly number[];
  details: (index: number) => PlotDetails;
  children: (index?: number) => ReactNode;
}) {
  const id = useId();
  const [focus, setFocus] = useState<{ index: number; nearTop: boolean }>();
  const active = focus && positions[focus.index] !== undefined ? focus : undefined;
  const tooltip = active && details(active.index);

  function select(index: number, nearTop: boolean) {
    if (positions.length === 0) return;
    const next = Math.max(0, Math.min(positions.length - 1, index));
    setFocus(current => current?.index === next && current.nearTop === nearTop ? current : { index: next, nearTop });
  }

  function pointAt(event: PointerEvent<HTMLDivElement>) {
    const box = event.currentTarget.getBoundingClientRect();
    if (box.width <= 0) return;
    const x = (event.clientX - box.left) / box.width;
    let nearest = 0;
    for (let index = 1; index < positions.length; index++) {
      if (Math.abs(x - positions[index]) < Math.abs(x - positions[nearest])) nearest = index;
    }
    select(nearest, event.clientY - box.top < box.height / 2);
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    const index = active?.index ?? 0;
    if (event.key === 'Escape') setFocus(undefined);
    else if (event.key === 'ArrowRight') select(active ? index + 1 : index, false);
    else if (event.key === 'ArrowLeft') select(active ? index - 1 : index, false);
    else if (event.key === 'Home') select(0, false);
    else if (event.key === 'End') select(positions.length - 1, false);
    else return;
    event.preventDefault();
  }

  return <div className={css.plot} tabIndex={0} role={label ? 'group' : undefined} aria-label={label} aria-describedby={tooltip ? id : undefined}
    onPointerMove={event => { if (event.pointerType !== 'touch') pointAt(event); }}
    onPointerDown={event => { if (event.pointerType === 'touch') pointAt(event); }}
    onPointerLeave={event => { if (event.pointerType !== 'touch') setFocus(undefined); }}
    onBlur={() => setFocus(undefined)} onKeyDown={onKeyDown}>
    {children(active?.index)}
    {active && tooltip && <div id={id} role="tooltip" className={css.tooltip} data-bottom={active.nearTop || undefined}
      style={{ '--plot-position': `${positions[active.index] * 100}%` } as CSSProperties}>
      <strong>{tooltip.title}</strong>
      <dl>
        {tooltip.rows.map((row, index) => <div className={css.fact} key={index} data-emphasis={row.emphasis || undefined}>
          <dt>{row.color && <span className={css.swatch} style={{ backgroundColor: row.color }}/>}
            {row.label}
          </dt>
          <dd>{row.value}</dd>
        </div>)}
      </dl>
    </div>}
  </div>;
}
