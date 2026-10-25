import { createContext, memo, useContext } from 'react';
import { Handle, Position, type Node, type NodeProps } from '@xyflow/react';
import clsx from 'clsx';
import type { NodeKind } from '../../document/model';
import type { ControlPort } from '../../document/ports';
import { KindIcon } from '../icons';
import css from './Editor.module.css';

export interface WorkflowNodeData extends Record<string, unknown> {
  readonly kind: NodeKind;
  readonly kindLabel: string;
  readonly summary: string;
  readonly entryLabel?: string;
  readonly ports: readonly { port: ControlPort; label: string; free: boolean; addLabel: string }[];
  /** `error`: a structural rejection here; `warning`: a dependency problem;
   * `nested`: a problem inside a block this node owns. */
  readonly problem?: 'error' | 'warning' | 'nested';
  readonly problemLabel: string;
}
export type WorkflowFlowNode = Node<WorkflowNodeData, 'workflow'>;

/** Stable canvas callbacks, provided once so node renders never depend on
 * fresh closures. */
export const CanvasActions = createContext<{ addAfter(node: string, port: ControlPort, anchor: HTMLElement): void }>({ addAfter() {} });

const PORT_SPACING = 30;

/** A restrained native node card: id, kind and one-line summary, with exactly
 * its native ports. A Return has no outgoing handle. */
export const WorkflowNode = memo(function WorkflowNode({ id, data, selected }: NodeProps<WorkflowFlowNode>) {
  const actions = useContext(CanvasActions);
  const multiple = data.ports.length > 1;
  return <div className={clsx(css.node, selected && css.nodeSelected, data.problem && css[`node_${data.problem}`])} title={data.problem ? data.problemLabel : undefined}>
    {data.entryLabel && <span className={css.entry}>{data.entryLabel}</span>}
    <Handle type="target" position={Position.Left} id="in" className={css.handle} aria-hidden="true" />
    <div className={css.nodeHead}>
      <span className={clsx(css.nodeIcon, css[`kind_${data.kind}`])}><KindIcon kind={data.kind} size={17} /></span>
      <span className={css.nodeText}><span className={css.nodeTitle}>{id}</span><span className={css.nodeType}>{data.kindLabel}</span></span>
    </div>
    <p className={css.nodeSummary}>{data.summary}</p>
    {data.ports.map(({ port, label, free, addLabel }, index) => {
      const top = multiple ? `calc(50% + ${(index - (data.ports.length - 1) / 2) * PORT_SPACING}px)` : '50%';
      return <div key={port}>
        <Handle type="source" position={Position.Right} id={port} className={css.handle} style={{ top }} aria-hidden="true" />
        {multiple && <span className={css.outcome} style={{ top }}>{label}</span>}
        {selected && free && <button type="button" className={clsx(css.addAfter, 'nodrag', 'nopan', multiple && css.addAfterLabeled)} style={{ top }} aria-label={addLabel}
          onClick={event => actions.addAfter(id, port, event.currentTarget)}>+</button>}
      </div>;
    })}
  </div>;
});
