import { memo, useContext, useState } from 'react';
import { BaseEdge, EdgeLabelRenderer, getBezierPath, type EdgeProps } from '@xyflow/react';
import clsx from 'clsx';
import { useTranslation } from '../../../../locale/react';
import type { ControlPort } from '../../document/ports';
import { IconPlus } from '../icons';
import { CanvasActions } from './WorkflowNode';
import type { SceneEdge } from './scene';
import css from './Editor.module.css';

/** As Dify's CustomEdge: a "+" at the midpoint, shown while the edge or the
 * button is hovered or focused, inserts a node into this connection. */
export const WorkflowEdge = memo(function WorkflowEdge({ id, source, sourceHandleId, sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition, selected, markerEnd, style, interactionWidth }: EdgeProps<SceneEdge>) {
  const tx = useTranslation(), actions = useContext(CanvasActions);
  const [path, x, y] = getBezierPath({ sourceX, sourceY, sourcePosition, targetX, targetY, targetPosition });
  const [hovered, setHovered] = useState(false), [near, setNear] = useState(false);
  return <>
    <g onMouseEnter={() => setHovered(true)} onMouseLeave={() => setHovered(false)}><BaseEdge id={id} path={path} markerEnd={markerEnd} style={style} interactionWidth={interactionWidth} /></g>
    {actions.editable && <EdgeLabelRenderer>
      <div className={clsx(css.edgeAdd, (hovered || near || selected) && css.edgeAddShown, 'nodrag nopan')} style={{ transform: `translate(-50%, -50%) translate(${x}px, ${y}px)` }}
        onMouseEnter={() => setNear(true)} onMouseLeave={() => setNear(false)} onFocus={() => setNear(true)} onBlur={() => setNear(false)}>
        <button type="button" className={css.handleAdd} aria-label={tx('workflows:canvas.insert-edge')} onClick={event => { event.stopPropagation(); actions.addAfter(source, sourceHandleId as ControlPort | 'entry', event.currentTarget); }}><IconPlus size={10} /></button>
      </div>
    </EdgeLabelRenderer>}
  </>;
});
