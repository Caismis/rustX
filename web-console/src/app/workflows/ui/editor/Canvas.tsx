import type { ArrayEdit } from '../../document/fields';
import { memo, useCallback, useEffect, useMemo, useRef, useState, type Dispatch, type SetStateAction } from 'react';
import { Background, BackgroundVariant, MiniMap, ReactFlow, applyNodeChanges, useReactFlow, useViewport, useNodesInitialized, type Connection, type EdgeChange, type NodeChange, type Viewport } from '@xyflow/react';
import clsx from 'clsx';
import { useTranslation } from '../../../../locale/react';
import { IconSearchOutline16 } from '../../../../presentation/primitives/icons';
import { writeClipboard } from '../../../../presentation/primitives/clipboard';
import type { FieldDraft } from '../../machines/editor';
import { CommandRejected, addBefore, addNode, deleteNodes, disconnect, freshIdentifier, insertNode } from '../../document/commands';
import { copyFragment, fragmentOwnsField, parseFragment, pasteFragment } from '../../document/clipboard';
import { freshNote, putNote, removeNotes, setNodePresentation } from '../../document/geometry';
import { blockAt, blockKey as keyOfBlock, deepNodeCount, edgesOf, sameBlockPath, sameDocument, type Block, type BlockPath, type NodeKind, type WorkflowDocument } from '../../document/model';
import { type ControlPort } from '../../document/ports';
import { IconCursor, IconFit, IconHand, IconMinus, IconPlus, IconBlock, IconFile, IconGrid, IconWorkflow } from '../icons';
import { LayoutSession, organizeDocument, type LayoutOwner, type Point } from './layout';
import { Palette } from './Palette';
import { applySceneConnection } from './connections';
import { readViewport, writeViewport } from './view-store';
import { CanvasActions, WorkflowNode } from './WorkflowNode';
import { NODE_WIDTH, projectScene, type SceneEdge, type SceneNode } from './scene';
import { WorkflowEdge } from './WorkflowEdge';
import { viewId, type Duplicate, type Selection } from './selection';
import css from './Editor.module.css';

export type { Selection } from './selection';
export interface Problems { readonly nodes: ReadonlyMap<string, 'error' | 'warning' | 'nested'>; readonly edges: ReadonlySet<string> }
type Edit = (apply: (document: WorkflowDocument) => WorkflowDocument, coalesce?: string, array?: ArrayEdit) => void;
const nodeTypes = { workflow: WorkflowNode }, edgeTypes = { workflow: WorkflowEdge };
const GAP = 96;
export function keyboardOwned(target: EventTarget | null): boolean {
  return !!(target as Element | null)?.closest?.('input, textarea, select, [contenteditable="true"], .cm-editor, [role="dialog"], [role="alertdialog"], [role="listbox"], [role="menu"]');
}
/** Where a chosen kind lands: free, after a free port, into an edge, at the
 * entry, or ahead of a node. `room` moves later nodes right to make space,
 * as Dify does when inserting between two close nodes. */
interface Addition {
  at: Point; position: Point; path: BlockPath; insertion?: boolean; room?: { ids: string[]; dx: number };
  from?: { node: string; port: ControlPort }; edge?: { index: number; value: Block['edges'] extends (infer T)[] | undefined ? T : never }; before?: string;
}

/** One React Flow scene, one native editing document. Local nodes retain only
 * transient pointer geometry/selection, never executable definitions. */
export const Canvas = memo(function Canvas({ document, path, viewKey, viewEpoch, selection, onSelect, problems, edit, editable, blockLabel, currentState, organizeEpoch = 0, collapsed, setCollapsed, drafts, onDuplicate }: {
  document: WorkflowDocument; path: BlockPath; viewKey: string; viewEpoch: number; selection: Selection; onSelect: (selection: Selection) => void;
  problems: Problems; edit: Edit; editable: boolean; blockLabel: string; currentState: () => LayoutOwner; organizeEpoch?: number;
  collapsed: ReadonlySet<string>; setCollapsed: Dispatch<SetStateAction<ReadonlySet<string>>>;
  drafts: Readonly<Record<string, FieldDraft>>; onDuplicate: Duplicate;
}) {
  const tx = useTranslation(), flow = useReactFlow<SceneNode, SceneEdge>(), initialized = useNodesInitialized(), container = useRef<HTMLDivElement>(null);
  const [stored] = useState(() => readViewport(viewKey));
  const [source] = useState(() => crypto.randomUUID());
  const [mode, setMode] = useState<'select' | 'pan'>('select'), [minimap, setMinimap] = useState(true);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [focusedId, setFocusedId] = useState<string>();
  const [palette, setPalette] = useState<Addition>(), [menu, setMenu] = useState<{ id: string; at: Point }>();
  const [search, setSearch] = useState<string>(), [message, setMessage] = useState<string>();
  const [organizing, setOrganizing] = useState(false), session = useRef(new LayoutSession());
  const [frameSize, setFrameSize] = useState('');
  useEffect(() => {
    const observer = new ResizeObserver(entries => {
      const { width, height } = entries[0].contentRect;
      setFrameSize(`${width}:${height}`);
    });
    observer.observe(container.current!);
    return () => observer.disconnect();
  }, []);
  const mutate = useCallback<Edit>((apply, coalesce, array) => { session.current.cancel(); edit(apply, coalesce, array); }, [edit]);
  useEffect(() => () => session.current.cancel(), []);
  const scope = selection && 'path' in selection ? selection.path ?? path : path;
  const selectedId = selection?.kind === 'node' ? viewId(scope, 'node', selection.id) : selection?.kind === 'start' ? viewId(scope, 'start') : selection?.kind === 'note' ? viewId(scope, 'note', selection.id) : undefined;
  // A located hidden child expands exactly its ancestors; it does not create a
  // second focused graph or discard the current panel's unapplied text.
  const selectionKey = JSON.stringify(selection);
  useEffect(() => {
    setSelected(current => selectedId && current.has(selectedId) ? current : new Set(selectedId ? [selectedId] : []));
  }, [selectedId]);
  useEffect(() => {
    if (!selection || !('path' in selection)) return;
    const selectedPath = selection.path ?? path;
    setCollapsed(current => {
      const next = new Set(current);
      for (let index = 0; index < selectedPath.length; index++) next.delete(viewId(selectedPath.slice(0, index), 'node', selectedPath[index].node));
      return next.size === current.size ? current : next;
    });
  }, [selectionKey, keyOfBlock(path)]);
  const scene = useMemo(() => projectScene(document, collapsed), [document, collapsed]);
  const projected = useMemo(() => scene.nodes.map(node => ({ ...node,
    // As Dify: ordinary cards size to their content. Only expanded containers
    // and notes have an explicit, user-resizable height.
    height: node.data.expanded || node.data.view === 'note' ? node.height : undefined,
    selected: selected.has(node.id) || node.id === selectedId,
    draggable: editable && node.draggable !== false, connectable: editable && node.connectable !== false,
    ...(node.parentId ? { extent: [[24, 64], [1_000_000, 1_000_000]] as [[number, number], [number, number]] } : {}),
    ariaLabel: node.data.view === 'node' ? tx('workflows:canvas.node-label', { p0: tx(`workflows:kind.${node.data.definition!.type}`), p1: `${node.data.local}${node.data.path.length ? ` · ${keyOfBlock(node.data.path)}` : ''}` })
      : node.data.view === 'start' ? tx(node.data.path.length ? 'workflows:canvas.nested-start-label' : 'workflows:canvas.start-label', { p0: keyOfBlock(node.data.path) })
      : tx('workflows:canvas.note-label', { p0: node.data.local }),
    className: problems.nodes.has(node.id) ? css[`node_${problems.nodes.get(node.id)}`] : undefined,
  })), [scene, selected, selectedId, editable, problems, tx]);
  const [nodes, setNodes] = useState<SceneNode[]>(projected);
  useEffect(() => setNodes(current => {
    const previous = new Map(current.map(node => [node.id, node]));
    // Selection and position changes keep React Flow's measured handle bounds.
    // Removing `measured` reinitializes every edge between the two clicks of a
    // double-click. Geometry/type changes still request fresh measurements.
    return projected.map(node => {
      const old = previous.get(node.id);
      return old && old.width === node.width && old.height === node.height && old.data.definition?.type === node.data.definition?.type
        ? { ...node, measured: old.measured } : node;
    });
  }), [projected]);
  const edges = useMemo(() => scene.edges.map(edge => {
    const selected = selection?.kind === 'edge' && sameBlockPath(selection.path ?? scope, edge.data!.path) && selection.index === edge.data!.index;
    return { ...edge, reconnectable: editable && selected, selected,
      className: clsx(css.edge, problems.edges.has(edge.id) && css.edgeError) };
  }), [scene, editable, selection, scope, problems]);
  const byId = useMemo(() => new Map(scene.nodes.map(node => [node.id, node])), [scene]);
  const choose = useCallback((node: SceneNode, multiple = false) => {
    const { view, local: id, path } = node.data;
    if (!multiple) setSelected(new Set([node.id]));
    onSelect(view === 'node' ? { kind: 'node', id, path } : view === 'note' ? { kind: 'note', id, path } : { kind: 'start', path });
  }, [onSelect]);
  const report = (cause: unknown) => setMessage(cause instanceof CommandRejected ? tx(`workflows:rejection.${cause.code}`) : tx('workflows:canvas.operation-failed'));

  const persist = useCallback((moved: readonly SceneNode[]) => {
    if (!editable || !moved.length) return;
    mutate(document => {
      let next = document;
      for (const item of moved) {
        const { path, local, view } = item.data;
        if (view === 'node') next = setNodePresentation(next, path, local, { position: item.position });
        else if (view === 'start') next = setNodePresentation(next, path, local, { position: item.position });
        else if (view === 'note') {
          const note = next.editor?.notes?.find(note => note.id === local);
          if (note) next = putNote(next, { ...note, position: item.position });
        }
      }
      return next;
    });
  }, [editable, mutate]);
  const dragged = useRef(false);
  const reconnecting = useRef<SceneEdge | undefined>(undefined);
  const onNodesChange = useCallback((changes: NodeChange<SceneNode>[]) => {
    setNodes(current => applyNodeChanges(changes.filter(change => change.type === 'position' || change.type === 'dimensions' || change.type === 'select'), current));
    const selections = changes.filter(change => change.type === 'select');
    if (selections.length) setSelected(current => {
      const next = new Set(current);
      for (const change of selections) if (change.type === 'select') { if (change.selected) next.add(change.id); else next.delete(change.id); }
      return next.size === current.size && [...next].every(id => current.has(id)) ? current : next;
    });
    for (const change of selections) if (change.type === 'select' && change.selected) { const node = byId.get(change.id); if (node) choose(node, true); }
    if (!dragged.current) persist(changes.flatMap(change => { if (change.type !== 'position') return []; const node = byId.get(change.id); return change.dragging === false && change.position && node ? [{ ...node, position: change.position }] : []; }));
  }, [byId, choose, persist]);
  const onEdgesChange = useCallback((changes: EdgeChange<SceneEdge>[]) => {
    for (const change of changes) if (change.type === 'select' && change.selected) { const edge = scene.edges.find(edge => edge.id === change.id); if (edge?.data) onSelect({ kind: 'edge', index: edge.data.index, path: edge.data.path }); }
  }, [scene.edges, onSelect]);

  const connectionCommand = useCallback((current: WorkflowDocument, connection: Connection, old?: SceneEdge) =>
    applySceneConnection(current, byId, connection, old), [byId]);
  const isValidConnection = useCallback((connection: Connection | SceneEdge) => {
    try { connectionCommand(document, { ...connection, sourceHandle: connection.sourceHandle ?? null, targetHandle: connection.targetHandle ?? null }, reconnecting.current); return true; } catch { return false; }
  }, [document, connectionCommand]);

  const openAt = useCallback((client: Point, position: Point, path: BlockPath, extra: Partial<Addition> = {}) => {
    const bounds = container.current!.getBoundingClientRect();
    setPalette({ at: { x: Math.max(8, Math.min(client.x - bounds.left, bounds.width - 270)), y: Math.max(8, Math.min(client.y - bounds.top, bounds.height - 370)) }, position, path, ...extra }); setMenu(undefined);
  }, []);
  const positionBelow = useCallback((path: BlockPath): Point => ({ x: 360, y: Math.max(80,
    ...scene.nodes.filter(node => sameBlockPath(node.data.path, path)).map(node => node.position.y + (node.height ?? 128) + 72)) }), [scene]);
  /** Nodes of `path` at or right of `to` move right when a new node at `x` would crowd it. */
  const room = useCallback((path: BlockPath, x: number, to: string): Addition['room'] => {
    const target = byId.get(viewId(path, 'node', to)), dx = target ? x + NODE_WIDTH + GAP - target.position.x : 0;
    return target && dx > 0 ? { dx, ids: scene.nodes.filter(node => node.data.view === 'node' && sameBlockPath(node.data.path, path) && node.position.x >= target.position.x - 1).map(node => node.data.local) } : undefined;
  }, [byId, scene]);
  /** The first spot at or below `position` that no card of `path` covers. */
  const vacant = useCallback((path: BlockPath, position: Point): Point => {
    let y = position.y, cover: SceneNode | undefined;
    while ((cover = scene.nodes.find(node => sameBlockPath(node.data.path, path) && position.x < node.position.x + (node.width ?? NODE_WIDTH) && position.x + NODE_WIDTH > node.position.x
      && y < node.position.y + (node.height ?? 128) + 24 && y + 128 + 24 > node.position.y))) y = cover.position.y + (cover.height ?? 128) + 48;
    return { x: position.x, y };
  }, [scene]);
  const openFor = useCallback((id: string, anchor: HTMLElement, port?: ControlPort) => {
    const node = byId.get(id); if (!node) return;
    const bounds = anchor.getBoundingClientRect(), path = node.data.path;
    if (!port) return openAt({ x: bounds.right, y: bounds.bottom }, { x: node.position.x + 360, y: node.position.y + 180 }, path);
    const client = { x: bounds.right + 8, y: bounds.top }, position = { x: node.position.x + (node.width ?? NODE_WIDTH) + GAP, y: node.position.y };
    openAt(client, vacant(path, position), path, { from: { node: node.data.local, port } });
  }, [byId, document, openAt, positionBelow, room, vacant]);
  const openBefore = useCallback((id: string, anchor: HTMLElement) => {
    const node = byId.get(id); if (!node || node.data.view !== 'node') return;
    const bounds = anchor.getBoundingClientRect();
    openAt({ x: bounds.left - 268, y: bounds.top }, vacant(node.data.path, { x: node.position.x - NODE_WIDTH - GAP, y: node.position.y }), node.data.path, { before: node.data.local, insertion: true });
  }, [byId, openAt, vacant]);
  const actions = useMemo(() => ({
    editable,
    addAfter: (id: string, port: ControlPort, anchor: HTMLElement) => { if (editable) openFor(id, anchor, port); },
    insertEdge: (id: string, anchor: HTMLElement) => {
      const edge = scene.edges.find(edge => edge.id === id), source = edge && byId.get(edge.source);
      if (!editable || !edge?.data || !source) return;
      const value = edgesOf(blockAt(document, edge.data.path)!)[edge.data.index];
      const rect = anchor.getBoundingClientRect(), position = { x: source.position.x + (source.width ?? NODE_WIDTH) + GAP, y: source.position.y };
      openAt({ x: rect.right, y: rect.top }, position, edge.data.path, { edge: { index: edge.data.index, value }, insertion: true, room: room(edge.data.path, position.x, value.to) });
    },
    addBefore: (id: string, anchor: HTMLElement) => { if (editable) openBefore(id, anchor); },
    addIn: (id: string, anchor: HTMLElement) => { if (editable) openFor(id, anchor); },
    menu(id: string, anchor: HTMLElement) { const rect = anchor.getBoundingClientRect(), bounds = container.current!.getBoundingClientRect(); setMenu({ id, at: { x: Math.max(8, Math.min(rect.left - bounds.left, bounds.width - 200)), y: Math.max(8, Math.min(rect.bottom - bounds.top, bounds.height - 180)) } }); setPalette(undefined); },
    collapse(id: string) { setCollapsed(current => { const next = new Set(current); if (next.has(id)) next.delete(id); else next.add(id); return next; }); },
    geometryStart() { session.current.cancel(); },
    resize(id: string, geometry: { x: number; y: number; width: number; height: number }) {
      if (!editable) return;
      const node = byId.get(id); if (!node) return;
      mutate(document => {
        const { path, local, view } = node.data, dimensions = { width: geometry.width, height: geometry.height }, position = { x: geometry.x, y: geometry.y };
        if (view === 'node') return setNodePresentation(document, path, local, { position, dimensions });
        if (view === 'note') { const note = document.editor?.notes?.find(note => note.id === local); return note ? putNote(document, { ...note, dimensions, position }) : document; }
        return document;
      });
    },
  }), [editable, openFor, openBefore, byId, mutate, scene.edges, document, openAt, room]);
  const selectedNodes = (override?: string) => {
    const all = override ? [byId.get(override)].filter((node): node is SceneNode => !!node) : scene.nodes.filter(node => selected.has(node.id) || node.id === selectedId);
    return all.filter(node => !all.some(parent => parent.id !== node.id && node.data.path.length > parent.data.path.length && sameBlockPath(node.data.path.slice(0, parent.data.path.length), parent.data.path) && node.data.path[parent.data.path.length].node === parent.data.local && parent.data.view === 'node'));
  };
  const fragmentFor = (override?: string) => {
    const chosen = selectedNodes(override).filter(node => node.data.view === 'node' || node.data.view === 'note');
    const path = chosen[0]?.data.path;
    if (!path || chosen.some(node => !sameBlockPath(node.data.path, path))) throw new CommandRejected('cross_scope');
    if (Object.values(drafts).some(draft => fragmentOwnsField(path, chosen.filter(node => node.data.view === 'node').map(node => node.data.local), chosen.filter(node => node.data.view === 'note').map(node => node.data.local), draft.address))) throw new CommandRejected('pending_input');
    return copyFragment(document, source, path, chosen.filter(node => node.data.view === 'node').map(node => node.data.local), chosen.filter(node => node.data.view === 'note').map(node => node.data.local));
  };
  const duplicate = (id?: string) => {
    if (!editable) return;
    try {
      const chosen = selectedNodes(id).filter(node => node.data.view === 'node' || node.data.view === 'note'), path = chosen[0]?.data.path;
      if (!path || chosen.some(node => !sameBlockPath(node.data.path, path))) throw new CommandRejected('cross_scope');
      const result = onDuplicate(path, chosen.filter(node => node.data.view === 'node').map(node => node.data.local), chosen.filter(node => node.data.view === 'note').map(node => node.data.local));
      if (result) {
        setSelected(new Set([...result.nodes.map(id => viewId(path, 'node', id)), ...result.notes.map(id => viewId(path, 'note', id))]));
        setFocusedId(undefined); container.current?.focus({ preventScroll: true });
      }
    } catch (error) { report(error); }
  };
  const remove = (override?: string) => {
    if (!editable) return;
    const chosen = selectedNodes(override);
    mutate(document => {
      let next = document;
      const groups = new Map<string, { path: BlockPath; ids: string[] }>();
      for (const node of chosen) if (node.data.view === 'node') { const key = keyOfBlock(node.data.path), group = groups.get(key) ?? { path: node.data.path, ids: [] }; group.ids.push(node.data.local); groups.set(key, group); }
      for (const group of [...groups.values()].sort((a, b) => b.path.length - a.path.length)) next = deleteNodes(next, group.path, group.ids);
      const notes = chosen.filter(node => node.data.view === 'note').map(node => node.data.local); if (notes.length) next = removeNotes(next, notes);
      if (!chosen.length && selection?.kind === 'edge') next = disconnect(next, scope, [selection.index]);
      return next;
    });
    setSelected(new Set()); onSelect(undefined);
  };
  const add = (kind: NodeKind) => {
    if (!palette || !editable) return;
    const chosen = palette; let added = '';
    mutate(document => {
      const block = blockAt(document, chosen.path); if (!block) throw new CommandRejected('missing_node');
      const id = freshIdentifier(block, kind); added = id;
      if (chosen.edge && !sameDocument(edgesOf(block)[chosen.edge.index], chosen.edge.value)) throw new CommandRejected('missing_edge');
      let next = chosen.edge ? insertNode(document, chosen.path, chosen.edge.index, kind, id)
        : chosen.before ? addBefore(document, chosen.path, kind, id, chosen.before) : addNode(document, chosen.path, kind, id, chosen.from && { from: chosen.from.node, port: chosen.from.port });
      if (deepNodeCount(next.block) > 256) throw new CommandRejected('limit_exceeded');
      // As Dify: a new node is titled by its kind, numbered after the first
      // of that kind in its block, and starts with the kind's description.
      const name = tx(`workflows:kind.${kind}`), same = Object.values(block.nodes).filter(node => node.type === kind).length;
      for (const moved of chosen.room?.ids ?? []) {
        const position = byId.get(viewId(chosen.path, 'node', moved))?.position;
        if (position) next = setNodePresentation(next, chosen.path, moved, { position: { x: position.x + chosen.room!.dx, y: position.y } });
      }
      return setNodePresentation(next, chosen.path, id, { position: chosen.position, title: same ? `${name} ${same + 1}` : name, description: tx(`workflows:kind-about.${kind}`) });
    });
    setPalette(undefined); if (added) onSelect({ kind: 'node', id: added, path: chosen.path });
  };
  const addBelow = () => {
    const bounds = container.current!.getBoundingClientRect();
    openAt({ x: bounds.left + 72, y: bounds.top + 140 }, positionBelow(scope), scope);
  };
  const addNote = () => {
    if (!editable) return;
    let id = '';
    mutate(document => { id = freshNote(document); return putNote(document, { id, block: [...scope], text: '', theme: 'amber', position: { x: 32, y: Math.max(280, ...scene.nodes.filter(node => sameBlockPath(node.data.path, scope)).map(node => node.position.y + (node.height ?? 128) + 48)) }, dimensions: { width: 280, height: 160 } }); });
    onSelect({ kind: 'note', id, path: scope });
  };
  const organize = useCallback(async () => {
    if (!editable) return;
    const owner = currentState(), document = owner.document; if (!document) return;
    setOrganizing(true); setMessage(undefined);
    try {
      const sizes = new Map(flow.getNodes().map(node => [node.id, { width: node.measured?.width ?? node.width ?? 256, height: node.measured?.height ?? node.height ?? 128 }]));
      const result = await session.current.run(owner, currentState, () => organizeDocument(document, sizes));
      if (result) edit(current => current === document && currentState().generation === owner.generation ? result : current);
    } catch { setMessage(tx('workflows:canvas.layout-failed')); }
    finally { setOrganizing(false); }
  }, [currentState, editable, flow, edit, tx]);
  const epoch = useRef(organizeEpoch);
  useEffect(() => { if (epoch.current !== organizeEpoch) { epoch.current = organizeEpoch; void organize(); } }, [organizeEpoch, organize]);

  const locateId = focusedId ?? selectedId;
  // Pan only the selected or keyboard-focused card into usable view; preserve the author's zoom.
  useEffect(() => {
    if (!locateId || !initialized) return;
    const frame = requestAnimationFrame(() => {
      const node = flow.getInternalNode(locateId), bounds = container.current?.getBoundingClientRect();
      if (!node || !bounds) return;
      const inspector = container.current?.closest('section')?.querySelector('aside')?.getBoundingClientRect();
      let usableBottom = inspector && inspector.left < bounds.right && inspector.right > bounds.left && inspector.top > bounds.top ? Math.min(bounds.bottom, inspector.top) : bounds.bottom;
      const screen = flow.flowToScreenPosition(node.internals.positionAbsolute), viewport = flow.getViewport();
      const width = Math.min((node.measured.width ?? 256) * viewport.zoom, bounds.width - 120);
      const dx = screen.x < bounds.left + 80 ? bounds.left + 80 - screen.x : screen.x + width > bounds.right - 32 ? bounds.right - 32 - screen.x - width : 0;
      const overview = container.current?.querySelector('.react-flow__minimap')?.getBoundingClientRect();
      if (overview?.height && screen.x + dx + width > overview.left && screen.x + dx < overview.right) usableBottom = Math.min(usableBottom, overview.top);
      const height = Math.min((node.measured.height ?? 128) * viewport.zoom, usableBottom - bounds.top - 110);
      const dy = screen.y < bounds.top + 70 ? bounds.top + 70 - screen.y : screen.y + height > usableBottom - 40 ? usableBottom - 40 - screen.y - height : 0;
      if (dx || dy) void flow.setViewport({ ...viewport, x: viewport.x + dx, y: viewport.y + dy });
    });
    return () => cancelAnimationFrame(frame);
  }, [locateId, initialized, flow, collapsed, viewEpoch, frameSize, minimap]);
  useEffect(() => { if (viewEpoch) setSelected(new Set()); }, [viewEpoch]);
  const found = search === undefined ? [] : projectScene(document).nodes.filter(node => `${node.data.title} ${node.data.local} ${keyOfBlock(node.data.path)}`.toLowerCase().includes(search.toLowerCase())).slice(0, 64);
  return <div ref={container} className={css.canvas} aria-label={tx('workflows:canvas.label')} role="region" tabIndex={0}
    onFocusCapture={event => { const target = event.target as HTMLElement; if (target.matches('.react-flow__node')) setFocusedId(target.dataset.id); }}
    onBlurCapture={event => { if ((event.target as Element).matches('.react-flow__node')) setFocusedId(undefined); }}
    onCopy={event => { if (keyboardOwned(event.target) || window.getSelection()?.toString()) return; try { event.clipboardData.setData('text/plain', fragmentFor()); event.preventDefault(); } catch (error) { report(error); } }}
    onPaste={event => { if (!editable || keyboardOwned(event.target)) return; event.preventDefault(); try { const fragment = parseFragment(event.clipboardData.getData('text/plain')); mutate(document => pasteFragment(document, source, scope, fragment).document); } catch (error) { report(error); } }}
    onKeyDown={event => {
      if (event.nativeEvent.isComposing || event.defaultPrevented || keyboardOwned(event.target) || window.getSelection()?.toString()) return;
      const mod = event.metaKey || event.ctrlKey, key = event.key.toLowerCase();
      const focused = (event.target as Element).closest<HTMLElement>('.react-flow__node');
      if ((event.key === 'Enter' || event.key === ' ') && focused) { const node = byId.get(focused.dataset.id ?? ''); if (node) { event.preventDefault(); choose(node); } }
      else if (event.key === 'Escape') { event.preventDefault(); setPalette(undefined); setMenu(undefined); setSearch(undefined); onSelect(undefined); container.current?.focus(); }
      else if (mod && key === 'd') { event.preventDefault(); if (editable) duplicate(); }
      else if (mod && key === 'f') { event.preventDefault(); setSearch(''); }
      else if (mod && key === 'a') { event.preventDefault(); setSelected(new Set(scene.nodes.filter(node => sameBlockPath(node.data.path, scope) && ['node', 'note'].includes(node.data.view)).map(node => node.id))); }
      else if (event.key === 'Delete' || event.key === 'Backspace') { event.preventDefault(); remove(); }
      else if (!mod && key === 'n' && editable) { event.preventDefault(); addBelow(); }
      else if (!mod && key === 'v') setMode('select'); else if (!mod && key === 'h') setMode('pan');
    }}>
    <CanvasActions.Provider value={actions}>
      <ReactFlow<SceneNode, SceneEdge> nodes={nodes} edges={edges} nodeTypes={nodeTypes} edgeTypes={edgeTypes} connectOnClick={false} onNodesChange={onNodesChange} onEdgesChange={onEdgesChange}
        onNodeDragStart={() => { dragged.current = true; session.current.cancel(); }} onNodeDragStop={(_, __, moved) => { dragged.current = false; persist(moved); }}
        onConnect={connection => { if (editable) mutate(document => connectionCommand(document, connection)); }} onReconnect={(edge, connection) => { if (editable) mutate(document => connectionCommand(document, connection, edge)); }}
        onReconnectStart={(_, edge) => { reconnecting.current = edge; }} onReconnectEnd={() => { reconnecting.current = undefined; }}
        isValidConnection={isValidConnection} elevateEdgesOnSelect edgesReconnectable={editable} nodesConnectable={editable} deleteKeyCode={null} nodesDraggable={editable}
        onNodeClick={(event, node) => choose(node, event.metaKey || event.ctrlKey || event.shiftKey)}
        onEdgeClick={(_, edge) => { container.current?.focus({ preventScroll: true }); if (!edge.data) return; setSelected(new Set()); onSelect({ kind: 'edge', index: edge.data.index, path: edge.data.path }); }}
        onNodeContextMenu={(event, node) => { event.preventDefault(); const bounds = container.current!.getBoundingClientRect(); setMenu({ id: node.id, at: { x: Math.min(event.clientX - bounds.left, bounds.width - 205), y: Math.min(event.clientY - bounds.top, bounds.height - 180) } }); }}
        onPaneClick={() => { setPalette(undefined); setMenu(undefined); onSelect(undefined); setSelected(new Set()); container.current?.focus({ preventScroll: true }); }}
        onPaneContextMenu={event => { if (!editable) return; event.preventDefault(); openAt({ x: event.clientX, y: event.clientY }, flow.screenToFlowPosition({ x: event.clientX, y: event.clientY }), []); }}
        onMoveEnd={(_, viewport: Viewport) => writeViewport(viewKey, viewport)}
        defaultViewport={stored ?? { x: 24, y: 120, zoom: 1 }} minZoom={0.05} maxZoom={2}
        autoPanOnNodeFocus={false} panOnDrag={mode === 'pan' ? true : [1, 2]} selectionOnDrag={mode === 'select'} panOnScroll proOptions={{ hideAttribution: true }} ariaLabelConfig={{ 'node.a11yDescription.default': tx('workflows:canvas.label') }}>
        <Background variant={BackgroundVariant.Dots} gap={18} size={1} className={css.dots} />
        {minimap && <MiniMap className={css.minimap} pannable zoomable ariaLabel={tx('workflows:canvas.minimap')} nodeClassName={css.miniNode} maskColor="transparent" />}
      </ReactFlow>
      <div className={css.rail} role="toolbar" aria-label={tx('workflows:canvas.tools')} aria-orientation="vertical">
        <Tool label={tx('workflows:canvas.select')} active={mode === 'select'} onClick={() => setMode('select')}><IconCursor /></Tool>
        <Tool label={tx('workflows:canvas.pan')} active={mode === 'pan'} onClick={() => setMode('pan')}><IconHand /></Tool>
        <Tool label={tx('workflows:canvas.add')} disabled={!editable} onClick={addBelow}><IconPlus /></Tool>
        <Tool label={tx('workflows:canvas.add-note')} disabled={!editable} onClick={addNote}><IconFile /></Tool>
        <Tool label={tx('workflows:canvas.organize')} disabled={!editable || organizing} onClick={() => void organize()}><IconWorkflow /></Tool>
        <Tool label={tx('workflows:canvas.find')} onClick={() => setSearch('')}><IconSearchOutline16 /></Tool>
        <Tool label={tx('workflows:canvas.fit')} onClick={() => void flow.fitView({ padding: 0.15, maxZoom: 1, duration: 150 })}><IconFit /></Tool>
        <Tool label={tx('workflows:canvas.block-settings')} active={selection?.kind === 'block'} onClick={() => onSelect({ kind: 'block' })}><IconBlock /></Tool>
        <Tool label={tx('workflows:canvas.minimap')} active={minimap} onClick={() => setMinimap(value => !value)}><IconGrid size={13} /></Tool>
      </div>
      <Zoom />
      <p className={css.canvasStatus} aria-live="polite">{message ?? (organizing ? tx('workflows:canvas.organizing') : tx('workflows:canvas.status', { p0: blockLabel, p1: deepNodeCount(document.block) }))}</p>
      {palette && <Palette at={palette.at} insertion={!!palette.insertion} onChoose={add} onClose={() => { setPalette(undefined); container.current?.focus(); }} />}
      {menu && <div className={css.contextMenu} role="dialog" aria-label={tx('workflows:canvas.node-actions', { p0: byId.get(menu.id)?.data.title ?? '' })} style={{ left: menu.at.x, top: menu.at.y }} onKeyDown={event => { if (event.key === 'Escape') { event.stopPropagation(); setMenu(undefined); container.current?.focus(); } }}>
        {['node', 'note'].includes(byId.get(menu.id)?.data.view ?? '') ? <>
          <button type="button" autoFocus onClick={() => { try { const text = fragmentFor(menu.id); void writeClipboard(text).then(ok => setMessage(tx(ok ? 'workflows:canvas.copied' : 'workflows:notice.copy-failed'))); } catch (error) { report(error); } setMenu(undefined); }}>{tx('workflows:canvas.copy')}</button>
          <button type="button" disabled={!editable} onClick={() => { duplicate(menu.id); setMenu(undefined); }}>{tx('workflows:canvas.duplicate')}</button>
          <button type="button" disabled={!editable} onClick={() => { remove(menu.id); setMenu(undefined); }}>{tx(byId.get(menu.id)?.data.view === 'note' ? 'workflows:note.delete' : 'workflows:inspector.delete-node')}</button>
        </> : <button type="button" disabled={!editable} onClick={event => openFor(menu.id, event.currentTarget)}>{tx('workflows:canvas.add')}</button>}
        <button type="button" onClick={() => { const node = byId.get(menu.id); if (node) choose(node); setMenu(undefined); }}>{tx('workflows:canvas.settings')}</button>
      </div>}
      {search !== undefined && <div className={css.nodeSearch} role="dialog" aria-label={tx('workflows:canvas.find')} onKeyDown={event => { if (event.key === 'Escape') { event.stopPropagation(); setSearch(undefined); container.current?.focus(); } }}>
        <input autoFocus className={css.input} aria-label={tx('workflows:canvas.find')} placeholder={tx('workflows:canvas.find')} value={search} onChange={event => setSearch(event.target.value)} />
        {found.map(node => <button type="button" key={node.id} onClick={() => { choose(node); setSearch(undefined); }}>{node.data.view === 'start' ? node.data.title || tx(node.data.path.length ? 'workflows:canvas.nested-start' : 'workflows:canvas.start-title') : node.data.title}<small>{keyOfBlock(node.data.path)}</small></button>)}
        {!found.length && <p className={css.note}>{tx('workflows:palette.none')}</p>}
      </div>}
    </CanvasActions.Provider>
  </div>;
});

import type { ReactNode } from 'react';
function Tool({ label, active, disabled, onClick, children }: { label: string; active?: boolean; disabled?: boolean; onClick: () => void; children: ReactNode }) {
  return <button type="button" className={clsx(css.tool, active && css.toolActive)} aria-pressed={active} aria-label={label} title={label} disabled={disabled} onClick={onClick}>{children}</button>;
}
function Zoom() {
  const tx = useTranslation(), flow = useReactFlow(), { zoom } = useViewport();
  return <div className={css.zoom} role="group" aria-label={tx('workflows:canvas.tools')}><button type="button" aria-label={tx('workflows:canvas.zoom-out')} onClick={() => void flow.zoomOut({ duration: 120 })}><IconMinus size={13} /></button><span>{Math.round(zoom * 100)}%</span><button type="button" aria-label={tx('workflows:canvas.zoom-in')} onClick={() => void flow.zoomIn({ duration: 120 })}><IconPlus size={13} /></button></div>;
}
