import { createContext, memo, useContext } from 'react';
import { Handle, NodeResizer, Position, type NodeProps, type ResizeParams } from '@xyflow/react';
import clsx from 'clsx';
import { useTranslation } from '../../../../locale/react';
import { IconEllipsisOutline16 } from '../../../../presentation/primitives/icons';
import { MarkdownText } from '../../../../presentation/markdown/MarkdownText';
import { outputPorts, type ControlPort } from '../../document/ports';
import { KindIcon, IconEntry, IconPlus } from '../icons';
import { nodeSummary } from './summary';
import type { SceneNode } from './scene';
import css from './Editor.module.css';

export const CanvasActions = createContext<{
  addAfter(id: string, port: ControlPort, anchor: HTMLElement): void;
  addIn(id: string, anchor: HTMLElement): void;
  menu(id: string, anchor: HTMLElement): void;
  collapse(id: string): void;
  resize(id: string, geometry: ResizeParams): void;
  geometryStart(): void;
}>({ addAfter() {}, addIn() {}, menu() {}, collapse() {}, resize() {}, geometryStart() {} });

export const WorkflowNode = memo(function WorkflowNode({ id, data, selected }: NodeProps<SceneNode>) {
  const tx = useTranslation(), actions = useContext(CanvasActions);
  const node = data.definition, ports = node ? outputPorts(node.type) : data.view === 'start' ? ['entry'] as const : [];
  const container = data.view === 'lane' || data.expanded;
  if (data.view === 'note' && data.note) return <div className={clsx(css.noteNode, css[`note_${data.note.theme}`], selected && css.nodeSelected)}>
    <NodeResizer isVisible={selected} minWidth={160} minHeight={88} maxWidth={8192} maxHeight={8192} onResizeStart={() => actions.geometryStart()} onResizeEnd={(_, geometry) => actions.resize(id, geometry)} />
    <header className={css.noteHead}><span>{tx('workflows:note.title')}</span><button type="button" className={clsx(css.iconButton, 'nodrag nopan')} aria-label={tx('workflows:canvas.node-actions', { p0: data.local })} onClick={event => actions.menu(id, event.currentTarget)}><IconEllipsisOutline16 /></button></header>
    <div className={clsx(css.notePreview, 'nowheel nodrag')}><MarkdownText text={data.note.text || tx('workflows:note.empty')} variant="compact" /></div>
  </div>;
  if (data.view === 'lane') return <div className={clsx(css.lane, selected && css.nodeSelected)}>
    <header className={css.containerHead}><span>{data.title}</span><button type="button" className={clsx(css.iconButton, 'nodrag nopan')} aria-label={tx('workflows:canvas.add-in', { p0: data.title })} onClick={event => actions.addIn(id, event.currentTarget)}><IconPlus size={14} /></button></header>
  </div>;
  return <div className={clsx(css.node, container && css.containerNode, selected && css.nodeSelected)} title={data.description}>
    {container && <NodeResizer isVisible={selected && data.minSize.width <= 8192 && data.minSize.height <= 8192} minWidth={data.minSize.width} minHeight={data.minSize.height} maxWidth={8192} maxHeight={8192} onResizeStart={() => actions.geometryStart()} onResizeEnd={(_, geometry) => actions.resize(id, geometry)} />}
    {node && <Handle type="target" position={Position.Left} id="in" className={css.handle} style={container ? { top: 28 } : undefined} aria-hidden="true" />}
    <div className={clsx(css.nodeHead, container && css.containerHead)}>
      <span className={clsx(css.nodeIcon, node ? css[`kind_${node.type}`] : css.startIcon)}>{node ? <KindIcon kind={node.type} size={17} /> : <IconEntry size={17} />}</span>
      <span className={css.nodeText}><span className={css.nodeTitle}>{data.view === 'start' ? tx(data.path.length ? 'workflows:canvas.nested-start' : 'workflows:canvas.start') : data.title}</span><span className={css.nodeType}>{node ? tx(`workflows:kind.${node.type}`) : tx(data.path.length ? 'workflows:panel.body-input' : 'workflows:panel.arguments')}</span></span>
      {node && (node.type === 'loop' || node.type === 'parallel') && <button type="button" className={clsx(css.iconButton, 'nodrag nopan')} aria-label={tx(data.expanded ? 'workflows:canvas.collapse' : 'workflows:canvas.expand', { p0: data.title })} onClick={event => { event.stopPropagation(); actions.collapse(id); }}>{data.expanded ? '−' : '+'}</button>}
      <button type="button" className={clsx(css.iconButton, css.nodeMenu, 'nodrag nopan')} aria-label={tx('workflows:canvas.node-actions', { p0: data.local || tx('workflows:canvas.start') })} onClick={event => { event.stopPropagation(); data.view === 'start' ? actions.addIn(id, event.currentTarget) : actions.menu(id, event.currentTarget); }}><IconEllipsisOutline16 /></button>
    </div>
    {!container && <>
      {node && node.type !== 'branch' && <p className={css.nodeSummary}>{nodeSummary(tx, node)}</p>}
      {node?.type === 'branch' && <div className={css.branchConditions}><div><strong>{tx('workflows:canvas.if')}</strong><span>{nodeSummary(tx, node)}</span></div><div><strong>{tx('workflows:canvas.else')}</strong></div></div>}
      {!!data.fields?.length && <div className={css.nodeFields}>{data.fields.map(field => <div key={field.name}><b aria-hidden>{'{x}'}</b><span>{field.name}</span>{data.view === 'start' && field.required && <small>{tx('workflows:schema.required')}</small>}<code>{field.type}</code></div>)}</div>}
      {/* As Dify's NodeDescription: after the body, so ports keep their places. */}
      {node && data.description?.trim() && <p className={css.nodeDescription}>{data.description}</p>}
    </>}
    {ports.map((port, index) => {
      const top = container ? 28 + index * 30 : node?.type === 'branch' ? 94 + index * 46 : ports.length > 1 ? 52 + index * 36 : '50%';
      return <div key={port}><Handle type="source" position={Position.Right} id={port} className={css.handle} style={{ top }} aria-hidden="true" />
        {node && ports.length > 1 && <span className={css.outcome} style={{ top }}>{tx(`workflows:port.${port as ControlPort}`)}</span>}
        {selected && node && <button type="button" className={clsx(css.addAfter, ports.length > 1 && css.addAfterLabeled, 'nodrag nopan')} style={{ top }} aria-label={tx('workflows:canvas.add-after', { p0: data.local, p1: tx(`workflows:port.${port as ControlPort}`) })} onClick={event => { event.stopPropagation(); actions.addAfter(id, port as ControlPort, event.currentTarget); }}><IconPlus size={12} /></button>}
      </div>;
    })}
  </div>;
});
