import { useEffect, useRef } from 'react';
import type { WorkflowToolChoice } from '../../../../../../protocol/app-server/v62';
import { useTranslation } from '../../../../locale/react';
import type { NodeKind } from '../../document/model';
import { NodeChooser, type NodeChoice } from './NodeChooser';
import css from './Editor.module.css';

/** The contextual Add Node palette: Dify's block selector at the pointer,
 * without the kinds its block cannot hold. It closes on choice, Escape or
 * outside focus. */
export function Palette({ at, tools, omit, onChoose, onClose, insertion = false }: { at: { x: number; y: number }; tools: readonly WorkflowToolChoice[]; omit: readonly NodeKind[]; onChoose: (choice: NodeChoice) => void; onClose: () => void; insertion?: boolean }) {
  const tx = useTranslation();
  const root = useRef<HTMLDivElement>(null), input = useRef<HTMLInputElement>(null);
  // Take the keyboard after the opening gesture has settled.
  useEffect(() => { const frame = requestAnimationFrame(() => input.current?.focus()); return () => cancelAnimationFrame(frame); }, []);
  useEffect(() => {
    const outside = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) onClose(); };
    document.addEventListener('pointerdown', outside, true);
    return () => document.removeEventListener('pointerdown', outside, true);
  }, [onClose]);
  return <div ref={root} className={css.palette} style={{ left: at.x, top: at.y }} role="dialog" aria-label={tx('workflows:canvas.add')}
    onKeyDown={event => { if (event.key === 'Escape' && !event.nativeEvent.isComposing) { event.stopPropagation(); onClose(); } }}>
    {insertion && <p className={css.note}>{tx('workflows:canvas.insertion-note')}</p>}
    <NodeChooser tools={tools} input={input} omit={insertion ? [...omit, 'branch', 'loop_end', 'return'] : omit} onChoose={choice => { if (!('id' in choice)) onChoose(choice); }} />
  </div>;
}
