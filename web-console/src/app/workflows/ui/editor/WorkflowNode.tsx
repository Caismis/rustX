import { createContext, memo, useContext } from 'react';
import { Handle, NodeResizer, Position, useNodeConnections, type NodeProps, type ResizeParams } from '@xyflow/react';
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
  editable: boolean;
  addAfter(id: string, port: ControlPort, anchor: HTMLElement): void;
  insertEdge(id: string, anchor: HTMLElement): void;
  addBefore(id: string, anchor: HTMLElement): void;
  addIn(id: string, anchor: HTMLElement): void;
  menu(id: string, anchor: HTMLElement): void;
  collapse(id: string): void;
  resize(id: string, geometry: ResizeParams): void;
  geometryStart(): void;
}>({ editable: false, addAfter() {}, insertEdge() {}, addBefore() {}, addIn() {}, menu() {}, collapse() {}, resize() {}, geometryStart() {} });

export const WorkflowNode = memo(function WorkflowNode({ id, data, selected, isConnectable }: NodeProps<SceneNode>) {
  const tx = useTranslation(), actions = useContext(CanvasActions), connections = useNodeConnections();
  const node = data.definition, ports = node ? outputPorts(node.type) : data.view === 'start' ? ['next'] as const : [];
  const container = data.expanded;
  if (data.view === 'note' && data.note) return <div className={clsx(css.noteNode, css[`note_${data.note.theme}`], selected && css.nodeSelected)}>
    <NodeResizer isVisible={selected} minWidth={160} minHeight={88} maxWidth={8192} maxHeight={8192} onResizeStart={() => actions.geometryStart()} onResizeEnd={(_, geometry) => actions.resize(id, geometry)} />
    <header className={css.noteHead}><span>{tx('workflows:note.title')}</span><button type="button" className={clsx(css.iconButton, 'nodrag nopan')} aria-label={tx('workflows:canvas.node-actions', { p0: data.local })} onClick={event => actions.menu(id, event.currentTarget)}><IconEllipsisOutline16 /></button></header>
    <div className={clsx(css.notePreview, 'nowheel nodrag')}><MarkdownText text={data.note.text || tx('workflows:note.empty')} variant="compact" /></div>
  </div>;
  const adding = isConnectable && actions.editable, entered = connections.some(connection => connection.target === id);
  const card = <div className={clsx(css.node, container && css.containerNode, selected && css.nodeSelected)} title={data.description}>
    {container && <NodeResizer isVisible={selected && data.minSize.width <= 8192 && data.minSize.height <= 8192} minWidth={data.minSize.width} minHeight={data.minSize.height} maxWidth={8192} maxHeight={8192} onResizeStart={() => actions.geometryStart()} onResizeEnd={(_, geometry) => actions.resize(id, geometry)} />}
    {/* As Dify's NodeTargetHandle: "+" adds a node ahead of an unconnected input. */}
    {node && <Handle type="target" position={Position.Left} id="in" className={clsx(css.handle, entered && css.handleConnected)} style={{ top: 28 }}>
      {adding && !entered && <button type="button" className={clsx(css.handleAdd, 'nodrag nopan')} aria-label={tx('workflows:canvas.add-before', { p0: data.local })} onClick={event => { event.stopPropagation(); actions.addBefore(id, event.currentTarget); }}><IconPlus size={10} /></button>}
    </Handle>}
    <div className={clsx(css.nodeHead, container && css.containerHead)}>
      <span className={clsx(css.nodeIcon, node ? css[`kind_${node.type}`] : css.startIcon)}>{node ? <KindIcon kind={node.type} size={17} /> : <IconEntry size={17} />}</span>
      <span className={css.nodeText}><span className={css.nodeTitle}>{data.view === 'start' ? data.title || tx(data.path.length ? 'workflows:canvas.nested-start' : 'workflows:canvas.start-title') : data.title}</span><span className={css.nodeType}>{node ? tx(`workflows:kind.${node.type}`) : tx(data.path.length ? 'workflows:panel.body-input' : 'workflows:panel.arguments')}</span></span>
      {node && node.type === 'loop' && <button type="button" className={clsx(css.iconButton, 'nodrag nopan')} aria-label={tx(data.expanded ? 'workflows:canvas.collapse' : 'workflows:canvas.expand', { p0: data.title })} onClick={event => { event.stopPropagation(); actions.collapse(id); }}>{data.expanded ? '−' : '+'}</button>}
      <button type="button" className={clsx(css.iconButton, css.nodeMenu, 'nodrag nopan')} aria-label={tx('workflows:canvas.node-actions', { p0: data.local || tx('workflows:canvas.start') })} onClick={event => { event.stopPropagation(); data.view === 'start' ? actions.addIn(id, event.currentTarget) : actions.menu(id, event.currentTarget); }}><IconEllipsisOutline16 /></button>
    </div>
    {!container && <>
      {node && node.type !== 'branch' && <p className={css.nodeSummary}>{nodeSummary(tx, node)}</p>}
      {node?.type === 'branch' && <div className={css.branchConditions}><div><strong>{tx('workflows:canvas.if')}</strong><span>{nodeSummary(tx, node)}</span></div><div><strong>{tx('workflows:canvas.else')}</strong></div></div>}
      {!!data.fields?.length && <div className={css.nodeFields}>{data.fields.map(field => <div key={field.name}><b aria-hidden>{'{x}'}</b><span>{field.name}</span>{data.view === 'start' && field.required && <small>{tx('workflows:schema.required')}</small>}<code>{field.type}</code></div>)}</div>}
      {/* As Dify's NodeDescription: after the body, so ports keep their places. */}
      {(node || data.view === 'start') && data.description?.trim() && <p className={css.nodeDescription}>{data.description}</p>}
    </>}
    {ports.map((port, index) => {
      const top = node?.type === 'branch' ? 94 + index * 46 : ports.length > 1 && !container ? 52 + index * 36 : 28 + index * 30;
      const label = node ? tx(`workflows:port.${port as ControlPort}`) : tx('workflows:canvas.start');
      // As Dify's NodeSourceHandle: click "+" to add a node here, drag to connect.
      return <Handle key={port} type="source" position={Position.Right} id={port} className={clsx(css.handle, connections.some(connection => connection.source === id && connection.sourceHandle === port) && css.handleConnected)} style={{ top }}>
        {node && ports.length > 1 && <span className={css.outcome}>{label}</span>}
        {adding && <>
          <span className={css.handleTip} role="tooltip"><span><b>{tx('workflows:canvas.handle-click')}</b>{tx('workflows:canvas.handle-click-detail')}</span><span><b>{tx('workflows:canvas.handle-drag')}</b>{tx('workflows:canvas.handle-drag-detail')}</span></span>
          <button type="button" className={clsx(css.handleAdd, 'nodrag nopan')} aria-label={tx('workflows:canvas.add-after', { p0: data.local || tx('workflows:canvas.start'), p1: label })} onClick={event => { event.stopPropagation(); actions.addAfter(id, port, event.currentTarget); }}><IconPlus size={10} /></button>
        </>}
      </Handle>;
    })}
  </div>;
  // As Dify's EntryNodeContainer: the Workflow's Start carries a badge above it.
  return data.view === 'start' && !data.path.length ? <div className={css.entryFrame}><span className={css.entryBadge}>{tx('workflows:canvas.start')}</span>{card}</div> : card;
});
