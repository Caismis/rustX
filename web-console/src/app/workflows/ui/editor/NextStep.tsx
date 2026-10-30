import { branchPortLabel } from '../../document/branch';
import { autoUpdate, flip, offset, shift, useFloating } from '@floating-ui/react-dom';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import clsx from 'clsx';
import { useTranslation } from '../../../../locale/react';
import type { WorkflowToolChoice } from '../../../../../../protocol/app-server/v62';
import { IconCloseOutline16 } from '../../../../presentation/primitives/icons';
import { addNode, connect, disconnect, freshIdentifier } from '../../document/commands';
import { nodePresentation, setNodePresentation } from '../../document/geometry';
import { blockAt, edgesOf, nodeAt, startOf, sameBlockPath, type BlockPath, type NodeKind, type WorkflowDocument } from '../../document/model';
import { edgePort, portsOf, type ControlPort, type BuiltinPort } from '../../document/ports';
import { IconBlock, IconEntry, IconPlus, KindIcon } from '../icons';
import { NODE_WIDTH, projectScene } from './scene';
import { viewId, type Selection } from './selection';
import { chosenNode, NodeChooser, type Choice } from './NodeChooser';
import type { Edit } from './panels/shared';
import css from './Editor.module.css';

type Port = ControlPort;
/** The vertical middle of the 34px source tile. */
const SOURCE_MIDDLE = 17;

/** Dify's Line: straight to the first port, then curving down to each later
 * one, with a short tick where each line meets a port. */
function NextLine({ anchors }: { anchors: readonly number[] }) {
  const y = SOURCE_MIDDLE;
  return <svg className={css.nextLine} width={24} height={(anchors.at(-1) ?? y) + 4} aria-hidden="true">
    <rect x={0} y={y - 2} width={1} height={4} />
    {anchors.map((anchor, index) => {
      const end = index ? anchor : y;
      return <g key={index}>
        <path d={index ? `M0,${y} Q12,${y} 12,${y + 10} L12,${end - 10} Q12,${end} 24,${end}` : `M0,${y} L24,${y}`} />
        <rect x={23} y={end - 2} width={1} height={4} />
      </g>;
    })}
  </svg>;
}


/** The inspector's continuation uses the same native graph commands as the
 * canvas. Every addition appends a sibling without replacing existing edges. */
export function NextStep({ document, block, source, tools, edit, onSelect }: {
  document: WorkflowDocument; block: BlockPath; source?: string; tools: readonly WorkflowToolChoice[]; edit: Edit; onSelect: (selection: Selection) => void;
}) {
  const tx = useTranslation(), owner = blockAt(document, block)!;
  const sourceId = source ?? startOf(owner);
  const sourceNode = sourceId ? nodeAt(owner, sourceId) : undefined;
  const ports: readonly Port[] = sourceNode ? portsOf(sourceNode) : [];
  const [openPort, setOpenPort] = useState<Port>();
  const trigger = useRef<HTMLButtonElement>(null), panel = useRef<HTMLDivElement>(null), input = useRef<HTMLInputElement>(null);
  const { refs, floatingStyles, isPositioned } = useFloating({
    open: !!openPort, placement: 'left-start', strategy: 'fixed', whileElementsMounted: autoUpdate,
    middleware: [offset(8), flip({ fallbackPlacements: ['right-start'], crossAxis: false }), shift({ padding: 12 })],
  });
  // As Dify's next-step Line: one SVG connector from the source tile to each
  // port, meeting a labelled port at its label, as Dify's branch rows do.
  const portList = useRef<HTMLDivElement>(null), [anchors, setAnchors] = useState<number[]>([]);
  useLayoutEffect(() => {
    const element = portList.current;
    if (!element) return;
    const measure = () => setAnchors(current => {
      const next = [...element.children].map(child => {
        const label = child.querySelector<HTMLElement>('[data-port-label]');
        return Math.round(label ? label.offsetTop + label.offsetHeight / 2 : (child as HTMLElement).offsetTop + SOURCE_MIDDLE);
      });
      return next.length === current.length && next.every((value, index) => value === current[index]) ? current : next;
    });
    measure();
    // Size changes move the ports; measure only then, never on every render.
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [ports.length]);
  const close = (restoreFocus = false) => { setOpenPort(undefined); if (restoreFocus) trigger.current?.focus(); };
  useEffect(() => {
    if (!openPort) return;
    const frame = requestAnimationFrame(() => input.current?.focus());
    const outside = (event: PointerEvent) => {
      if (!panel.current?.contains(event.target as Node) && !trigger.current?.contains(event.target as Node)) setOpenPort(undefined);
    };
    window.document.addEventListener('pointerdown', outside);
    return () => { cancelAnimationFrame(frame); window.document.removeEventListener('pointerdown', outside); };
  }, [openPort]);
  if (!ports.length) return null;

  const targetsFor = (port: Port) => edgesOf(owner).filter(edge => edge.from === sourceId && edgePort(edge) === port).map(edge => edge.to);
  // An Agent route is named by its own ID, as the child answers with it.
  const portLabel = (port: Port) => sourceNode?.type === 'branch' ? branchPortLabel(sourceNode, port)
    : sourceNode?.type === 'agent' && sourceNode.routes?.some(route => route.id === port) ? port : tx(`workflows:port.${port as BuiltinPort}`);
  function nodeTitle(id: string) {
    return nodePresentation(document, block, id)?.title || id;
  }
  function open(port: Port, element: HTMLButtonElement) {
    if (openPort === port) { close(true); return; }
    trigger.current = element;
    refs.setReference(element);
    setOpenPort(port);
  }
  function remove(port: Port, target: string) {
    edit(current => {
      const index = edgesOf(blockAt(current, block)!).findIndex(edge => edge.from === sourceId && edgePort(edge) === port && edge.to === target);
      return index < 0 ? current : disconnect(current, block, [index]);
    });
  }
  function choose(choice: Choice) {
    if (!openPort || !sourceId) return;
    const port = openPort;
    if ('id' in choice) {
      edit(current => connect(current, block, sourceId, port, choice.id));
      close(); return;
    }
    let added = '';
    edit(current => {
      const owner = blockAt(current, block)!;
      const id = freshIdentifier(owner, choice.kind);
      const next = addNode(current, block, choice.kind, id, { from: sourceId, port });
      const scene = projectScene(current).nodes;
      const origin = scene.find(node => node.id === viewId(block, sourceNode?.type === 'start' ? 'start' : 'node', sourceNode?.type === 'start' ? undefined : sourceId));
      const siblings = new Set(edgesOf(owner).filter(edge => edge.from === sourceId).map(edge => edge.to));
      const outgoing = scene.filter(node => node.data.view === 'node' && sameBlockPath(node.data.path, block) && siblings.has(node.data.local));
      const position = { x: (origin?.position.x ?? 0) + (origin?.width ?? NODE_WIDTH) + 96,
        y: outgoing.length ? Math.max(...outgoing.map(node => node.position.y + (node.height ?? 128))) + 48 : origin?.position.y ?? 80 };
      while (scene.some(node => sameBlockPath(node.data.path, block) && position.x < node.position.x + (node.width ?? NODE_WIDTH)
        && position.x + NODE_WIDTH > node.position.x && position.y < node.position.y + (node.height ?? 128) + 24 && position.y + 152 > node.position.y)) position.y += 176;
      const made = chosenNode(tx, next, block, id, choice);
      added = id;
      return setNodePresentation(made.document, block, id, { position, title: made.title, description: made.description });
    });
    close();
    if (added) onSelect({ kind: 'node', id: added, path: block });
  }

  return <section className={css.nextSection} aria-label={tx('workflows:inspector.next-step')}>
    <h3 className={css.nextHeading}>{tx('workflows:inspector.next-step')}</h3>
    <p className={css.nextHint}>{tx('workflows:next-step.hint')}</p>
    <div className={clsx(css.nextFlow, sourceNode?.type === 'branch' && css.branchNextFlow)}>
      <span className={clsx(css.nextSource, sourceNode?.type === 'start' ? css.startIcon : css[`kind_${sourceNode!.type}`])}>
        {sourceNode?.type === 'start' ? <IconEntry size={18} /> : <KindIcon kind={sourceNode!.type} size={18} />}
      </span>
      <NextLine anchors={anchors} />
      <div ref={portList} className={css.nextPorts}>
        {ports.map(port => {
          const targets = targetsFor(port);
          return <div key={port} className={clsx(css.nextPort, port === 'fail' && css.nextPortFail)}>
            {ports.length > 1 && <span className={css.nextPortLabel} data-port-label>{portLabel(port)}</span>}
            {targets.map(target => { const targetNode = nodeAt(owner, target); return <div key={target} className={css.nextConnected}>
              <button type="button" className={css.nextTarget} title={nodeTitle(target)} onClick={() => onSelect({ kind: 'node', id: target, path: block })}>
                <span className={clsx(css.nextTargetIcon, targetNode && css[`kind_${targetNode.type}`])}>{targetNode ? <KindIcon kind={targetNode.type} size={15} /> : <IconBlock size={15} />}</span>
                <span>{nodeTitle(target)}</span>
              </button>
              <button type="button" className={css.nextRemove} aria-label={tx('workflows:next-step.disconnect', { p0: nodeTitle(target) })} title={tx('workflows:next-step.disconnect', { p0: nodeTitle(target) })} onClick={() => remove(port, target)}><IconCloseOutline16 /></button>
            </div>; })}
            <button type="button" className={css.nextAdd} aria-label={tx(targets.length ? 'workflows:next-step.parallel' : 'workflows:next-step.choose', { p0: ports.length > 1 ? portLabel(port) : tx('workflows:inspector.next-step') })}
              aria-haspopup="dialog" aria-expanded={openPort === port}
              onPointerDown={event => { if (event.button === 0) { event.preventDefault(); event.stopPropagation(); open(port, event.currentTarget); } }}
              onClick={event => { if (event.detail === 0) open(port, event.currentTarget); }}>
              <span className={css.nextAddIcon}><IconPlus size={13} /></span>
              {tx(targets.length ? 'workflows:next-step.parallel-label' : 'workflows:next-step.choose-label')}
            </button>
          </div>;
        })}
      </div>
    </div>
    {openPort && createPortal(<div ref={element => { panel.current = element; refs.setFloating(element); }} className={css.nextPicker}
      style={{ ...floatingStyles, visibility: isPositioned ? 'visible' : 'hidden' }} role="dialog" aria-label={tx('workflows:next-step.picker')}
      onKeyDown={event => { if (event.key === 'Escape' && !event.nativeEvent.isComposing) { event.preventDefault(); event.stopPropagation(); close(true); } }}>
      <NodeChooser tools={tools} input={input} preview="left" onChoose={choose}
        existing={Object.keys(owner.nodes).filter(id => id !== sourceId && owner.nodes[id].type !== 'start' && (!openPort || !targetsFor(openPort).includes(id))).map(id => ({ id, title: nodeTitle(id), kind: owner.nodes[id].type as NodeKind }))} />
    </div>, window.document.body)}
  </section>;
}
