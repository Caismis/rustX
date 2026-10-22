import type { ArrayEdit } from '../../document/fields';
import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Background, BackgroundVariant, MiniMap, ReactFlow, applyNodeChanges, useReactFlow, useViewport,
  type Connection, type Edge as FlowEdge, type EdgeChange, type NodeChange, type OnDelete, type Viewport,
} from '@xyflow/react';
import clsx from 'clsx';
import { useTranslation } from '../../../../locale/react';
import { addNode, connect, deleteNodes, disconnect, freshIdentifier, reconnect } from '../../document/commands';
import { edgesOf, type Block, type BlockPath, type NodeKind, type WorkflowDocument } from '../../document/model';
import type { ViewPositions } from '../../machines/editor';
import { edgePort, outputPorts, type ControlPort } from '../../document/ports';
import { IconCursor, IconFit, IconHand, IconMinus, IconPlus, IconBlock } from '../icons';
import { after, layout, NODE_WIDTH, type Point } from './layout';
import type { InspectorTab } from './forms/NodeForms';
import { Palette } from './Palette';
import { nodeSummary } from './summary';
import { blockView, readViews, writeBlockView } from './view-store';
import { CanvasActions, WorkflowNode, type WorkflowFlowNode } from './WorkflowNode';
import css from './Editor.module.css';

/** What the inspector shows. A node selection may ask for the tab that
 * shows one of its fields. */
export type Selection = { kind: 'node'; id: string; tab?: InspectorTab } | { kind: 'edge'; index: number } | { kind: 'block' } | { kind: 'check' } | undefined;
export interface Problems { readonly nodes: ReadonlyMap<string, 'error' | 'warning' | 'nested'>; readonly edges: ReadonlySet<number> }
type Edit = (apply: (document: WorkflowDocument) => WorkflowDocument, coalesce?: string, array?: ArrayEdit) => void;

const nodeTypes = { workflow: WorkflowNode };

/** The interactive projection of exactly one lexical block. Nodes and edges
 * are derived from the document; dragging, panning and zooming change only
 * browser-local view state, never the file or its dirty state. */
export const Canvas = memo(function Canvas({ block, path, blockKey, viewKey, viewEpoch, selection, onSelect, problems, edit, onMove, editable, blockLabel }: {
  block: Block; path: BlockPath; blockKey: string; viewKey: string; viewEpoch: number; selection: Selection; onSelect: (selection: Selection) => void;
  problems: Problems; edit: Edit; onMove: (moved: { block: string; before: ViewPositions; after: ViewPositions }) => void; editable: boolean; blockLabel: string;
}) {
  const tx = useTranslation();
  const flow = useReactFlow<WorkflowFlowNode>();
  const container = useRef<HTMLDivElement>(null);
  const [stored, setStored] = useState(() => blockView(readViews(viewKey), blockKey));
  // `viewEpoch` advances when an undo or redo restored stored positions.
  useEffect(() => setStored(blockView(readViews(viewKey), blockKey)), [viewKey, blockKey, viewEpoch]);
  const positions = useMemo(() => layout(block, stored?.positions), [block, stored]);
  const [mode, setMode] = useState<'select' | 'pan'>('select');
  const [palette, setPalette] = useState<{ at: { x: number; y: number }; position: Point; from?: { node: string; port: ControlPort } }>();

  const projected = useMemo<WorkflowFlowNode[]>(() => Object.entries(block.nodes).map(([id, node]) => {
    const used = new Set(edgesOf(block).filter(edge => edge.from === id).map(edgePort));
    const kindLabel = tx(`workflows:kind.${node.type}`);
    return {
      // The focusable React Flow node itself carries the name, so keyboard
      // users reach and activate exactly what they hear.
      id, type: 'workflow', position: positions[id], width: NODE_WIDTH, ariaLabel: tx('workflows:canvas.node-label', { p0: kindLabel, p1: id }),
      selected: selection?.kind === 'node' && selection.id === id,
      data: {
        kind: node.type, kindLabel, summary: nodeSummary(tx, node),
        entryLabel: block.entry === id ? tx('workflows:canvas.entry-marker', { p0: id }) : undefined,
        ports: outputPorts(node.type).map(port => ({ port, label: tx(`workflows:port.${port}`), free: editable && !used.has(port), addLabel: tx('workflows:canvas.add-after', { p0: id, p1: tx(`workflows:port.${port}`) }) })),
        problem: problems.nodes.get(id), problemLabel: tx('workflows:canvas.diagnostic'),
      },
    };
  }), [block, positions, selection, problems, editable, tx]);
  const [nodes, setNodes] = useState(projected);
  useEffect(() => setNodes(projected), [projected]);

  const edges = useMemo<FlowEdge[]>(() => edgesOf(block).map((edge, index) => ({
    id: `e${index}`, source: edge.from, target: edge.to, sourceHandle: edgePort(edge), targetHandle: 'in', data: { index },
    selected: selection?.kind === 'edge' && selection.index === index,
    className: clsx(css.edge, problems.edges.has(index) && css.edgeError), reconnectable: editable,
  })), [block, selection, problems, editable]);

  // Pointer moves change only local positions. Selection is the editor's:
  // React Flow reports it for pointer and keyboard (Enter or Space on a
  // focused node or edge) alike, and the inspector follows.
  const onNodesChange = useCallback((changes: NodeChange<WorkflowFlowNode>[]) => {
    setNodes(current => applyNodeChanges(changes.filter(change => change.type === 'position' || change.type === 'dimensions'), current));
    const selected = changes.filter(change => change.type === 'select' && change.selected);
    if (selected.length === 1 && selected[0].type === 'select') onSelect({ kind: 'node', id: selected[0].id });
  }, [onSelect]);
  const onEdgesChange = useCallback((changes: EdgeChange<FlowEdge>[]) => {
    const selected = changes.filter(change => change.type === 'select' && change.selected);
    const index = selected.length === 1 && selected[0].type === 'select' ? Number(selected[0].id.slice(1)) : NaN;
    if (Number.isSafeInteger(index)) onSelect({ kind: 'edge', index });
  }, [onSelect]);
  // A completed drag persists once, at its end, and is one view step.
  const dragged = useRef<ViewPositions>({});
  const onNodeDragStart = useCallback((_: unknown, __: unknown, moving: readonly WorkflowFlowNode[]) => {
    dragged.current = Object.fromEntries(moving.map(node => [node.id, positions[node.id] ?? node.position]));
  }, [positions]);
  const persist = useCallback((moved: readonly WorkflowFlowNode[]) => {
    const before = dragged.current;
    dragged.current = {};
    const after = Object.fromEntries(moved.filter(node => Object.hasOwn(before, node.id)).map(node => [node.id, { x: node.position.x, y: node.position.y }]));
    if (!Object.entries(after).some(([id, point]) => point.x !== before[id].x || point.y !== before[id].y)) return;
    writeBlockView(viewKey, blockKey, view => ({ ...view, positions: { ...positions, ...view.positions, ...after } }));
    setStored(blockView(readViews(viewKey), blockKey));
    onMove({ block: blockKey, before: Object.fromEntries(Object.keys(after).map(id => [id, before[id]])), after });
  }, [viewKey, blockKey, positions, onMove]);
  const onDelete = useCallback<OnDelete<WorkflowFlowNode, FlowEdge>>(({ nodes: removed, edges: cut }) => {
    if (!editable || (!removed.length && !cut.length)) return;
    const ids = new Set(removed.map(node => node.id));
    // Edges are addressed by index in the current block; detached ones go
    // with their nodes. One deletion is one edit.
    const indexes = cut.map(edge => (edge.data as { index: number }).index).filter(index => {
      const edge = edgesOf(block)[index];
      return edge && !ids.has(edge.from) && !ids.has(edge.to);
    });
    edit(document => {
      let next = indexes.length ? disconnect(document, path, indexes) : document;
      if (ids.size) next = deleteNodes(next, path, [...ids]);
      return next;
    });
    onSelect(undefined);
  }, [block, edit, path, editable, onSelect]);
  const onConnect = useCallback((connection: Connection) => {
    if (editable && connection.sourceHandle) edit(document => connect(document, path, connection.source, connection.sourceHandle as ControlPort, connection.target));
  }, [edit, path, editable]);
  const onReconnect = useCallback((old: FlowEdge, connection: Connection) => {
    if (editable && connection.sourceHandle) edit(document => reconnect(document, path, (old.data as { index: number }).index, connection.source, connection.sourceHandle as ControlPort, connection.target));
  }, [edit, path, editable]);
  const isValidConnection = useCallback((connection: Connection | FlowEdge) => connection.source !== connection.target && connection.targetHandle === 'in', []);

  const openAt = useCallback((client: { x: number; y: number }, position: Point, from?: { node: string; port: ControlPort }) => {
    const bounds = container.current!.getBoundingClientRect();
    setPalette({ at: { x: Math.min(client.x - bounds.left, bounds.width - 260), y: Math.min(client.y - bounds.top, bounds.height - 360) }, position, from });
  }, []);
  const actions = useMemo(() => ({
    addAfter(node: string, port: ControlPort, anchor: HTMLElement) {
      const rect = anchor.getBoundingClientRect();
      openAt({ x: rect.right + 8, y: rect.top }, after(block, positions, node, port), { node, port });
    },
  }), [block, positions, openAt]);
  const add = (kind: NodeKind) => {
    if (!palette) return;
    const id = freshIdentifier(block, kind);
    writeBlockView(viewKey, blockKey, view => ({ ...view, positions: { ...positions, ...view.positions, [id]: palette.position } }));
    setStored(blockView(readViews(viewKey), blockKey));
    edit(document => addNode(document, path, kind, id, palette.from && { from: palette.from.node, port: palette.from.port }));
    setPalette(undefined);
    onSelect({ kind: 'node', id });
  };
  /** A toolbar addition goes below the block's nodes, never on top of one. */
  const addBelow = () => {
    const bounds = container.current!.getBoundingClientRect();
    const points = Object.values(positions);
    const position = points.length
      ? { x: Math.min(...points.map(point => point.x)), y: Math.max(...points.map(point => point.y)) + 170 }
      : flow.screenToFlowPosition({ x: bounds.left + bounds.width / 2, y: bounds.top + bounds.height / 2 });
    openAt({ x: bounds.left + 72, y: bounds.top + 140 }, position);
  };
  return <div ref={container} className={css.canvas} aria-label={tx('workflows:canvas.label')} role="region">
    <CanvasActions.Provider value={actions}>
      <ReactFlow<WorkflowFlowNode, FlowEdge> nodes={nodes} edges={edges} nodeTypes={nodeTypes}
        onNodesChange={onNodesChange} onEdgesChange={onEdgesChange} onNodeDragStart={onNodeDragStart} onNodeDragStop={(_, __, moved) => persist(moved)} onDelete={onDelete}
        onConnect={onConnect} onReconnect={onReconnect} isValidConnection={isValidConnection} edgesReconnectable={editable}
        nodesConnectable={editable} deleteKeyCode={editable ? ['Delete', 'Backspace'] : null} nodesDraggable
        onNodeClick={(_, node) => onSelect({ kind: 'node', id: node.id })} onEdgeClick={(_, edge) => onSelect({ kind: 'edge', index: (edge.data as { index: number }).index })}
        onPaneClick={() => { setPalette(undefined); onSelect(undefined); }}
        onPaneContextMenu={event => { if (!editable) return; event.preventDefault(); openAt({ x: event.clientX, y: event.clientY }, flow.screenToFlowPosition({ x: event.clientX, y: event.clientY })); }}
        onMoveEnd={(_, viewport: Viewport) => writeBlockView(viewKey, blockKey, view => ({ ...view, viewport }))}
        defaultViewport={stored?.viewport} fitView={!stored?.viewport} fitViewOptions={{ padding: 0.25, maxZoom: 1 }}
        minZoom={0.2} maxZoom={2} panOnDrag={mode === 'pan' ? true : [1, 2]} selectionOnDrag={mode === 'select'} panOnScroll
        proOptions={{ hideAttribution: true }} ariaLabelConfig={{ 'node.a11yDescription.default': tx('workflows:canvas.label') }}>
        <Background variant={BackgroundVariant.Dots} gap={18} size={1} className={css.dots} />
        <MiniMap className={css.minimap} pannable zoomable ariaLabel={tx('workflows:canvas.minimap')} nodeClassName={css.miniNode} maskColor="transparent" />
      </ReactFlow>
      <div className={css.rail} role="toolbar" aria-label={tx('workflows:canvas.tools')} aria-orientation="vertical">
        <button type="button" className={clsx(css.tool, mode === 'select' && css.toolActive)} aria-pressed={mode === 'select'} aria-label={tx('workflows:canvas.select')} title={tx('workflows:canvas.select')} onClick={() => setMode('select')}><IconCursor /></button>
        <button type="button" className={clsx(css.tool, mode === 'pan' && css.toolActive)} aria-pressed={mode === 'pan'} aria-label={tx('workflows:canvas.pan')} title={tx('workflows:canvas.pan')} onClick={() => setMode('pan')}><IconHand /></button>
        <button type="button" className={css.tool} aria-label={tx('workflows:canvas.add')} title={tx('workflows:canvas.add')} disabled={!editable} onClick={addBelow}><IconPlus /></button>
        <button type="button" className={css.tool} aria-label={tx('workflows:canvas.fit')} title={tx('workflows:canvas.fit')} onClick={() => void flow.fitView({ padding: 0.25, maxZoom: 1, duration: 150 })}><IconFit /></button>
        <button type="button" className={clsx(css.tool, selection?.kind === 'block' && css.toolActive)} aria-label={tx('workflows:canvas.block-settings')} title={tx('workflows:canvas.block-settings')} onClick={() => onSelect({ kind: 'block' })}><IconBlock /></button>
      </div>
      <Zoom />
      <p className={css.canvasStatus} aria-live="polite">{tx('workflows:canvas.status', { p0: blockLabel, p1: Object.keys(block.nodes).length })}</p>
      {palette && <Palette at={palette.at} onChoose={add} onClose={() => setPalette(undefined)} />}
    </CanvasActions.Provider>
  </div>;
});

/** The only component subscribed to the live viewport. */
function Zoom() {
  const tx = useTranslation();
  const flow = useReactFlow();
  const { zoom } = useViewport();
  return <div className={css.zoom} role="group" aria-label={tx('workflows:canvas.tools')}>
    <button type="button" aria-label={tx('workflows:canvas.zoom-out')} onClick={() => void flow.zoomOut({ duration: 120 })}><IconMinus size={13} /></button>
    <span aria-live="polite">{Math.round(zoom * 100)}%</span>
    <button type="button" aria-label={tx('workflows:canvas.zoom-in')} onClick={() => void flow.zoomIn({ duration: 120 })}><IconPlus size={13} /></button>
    <button type="button" aria-label={tx('workflows:canvas.fit')} onClick={() => void flow.fitView({ padding: 0.25, maxZoom: 1, duration: 150 })}><IconFit size={13} /></button>
  </div>;
}
