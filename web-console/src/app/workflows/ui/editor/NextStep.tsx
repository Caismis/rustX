import { autoUpdate, flip, offset, shift, useFloating } from '@floating-ui/react-dom';
import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import clsx from 'clsx';
import { useTranslation } from '../../../../locale/react';
import { IconCloseOutline16, IconSearchOutline16 } from '../../../../presentation/primitives/icons';
import { addNode, connect, disconnect, freshIdentifier } from '../../document/commands';
import { nodePresentation, setNodePresentation } from '../../document/geometry';
import { blockAt, edgesOf, nodeAt, startOf, sameBlockPath, type BlockPath, type NodeKind, type WorkflowDocument } from '../../document/model';
import { edgePort, outputPorts, type ControlPort } from '../../document/ports';
import { IconBlock, IconEntry, IconPlus, KindIcon } from '../icons';
import { NODE_WIDTH, projectScene } from './scene';
import { viewId, type Selection } from './selection';
import type { Edit } from './panels/shared';
import css from './Editor.module.css';

type Port = ControlPort;
type Tab = 'nodes' | 'tools' | 'existing';
type Choice = { kind: NodeKind; group: string } | { id: string };

/** The inspector's continuation uses the same native graph commands as the
 * canvas. Every addition appends a sibling without replacing existing edges. */
export function NextStep({ document, block, source, edit, onSelect }: {
  document: WorkflowDocument; block: BlockPath; source?: string; edit: Edit; onSelect: (selection: Selection) => void;
}) {
  const tx = useTranslation(), owner = blockAt(document, block)!;
  const sourceId = source ?? startOf(owner);
  const sourceNode = sourceId ? nodeAt(owner, sourceId) : undefined;
  const ports: readonly Port[] = sourceNode ? outputPorts(sourceNode.type) : [];
  const [openPort, setOpenPort] = useState<Port>();
  const [tab, setTab] = useState<Tab>('nodes');
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const trigger = useRef<HTMLButtonElement>(null), panel = useRef<HTMLDivElement>(null), input = useRef<HTMLInputElement>(null);
  const { refs, floatingStyles, isPositioned } = useFloating({
    open: !!openPort, placement: 'left-start', strategy: 'fixed', whileElementsMounted: autoUpdate,
    middleware: [offset(8), flip({ fallbackPlacements: ['right-start'], crossAxis: false }), shift({ padding: 12 })],
  });
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
  const portLabel = (port: Port) => tx(`workflows:port.${port}`);
  const groups: { label: string; kinds: readonly NodeKind[] }[] = [
    { label: tx('workflows:next-step.group-ai'), kinds: ['agent', 'model'] },
    { label: tx('workflows:next-step.group-control'), kinds: ['branch', 'loop'] },
    { label: tx('workflows:next-step.group-result'), kinds: ['review', 'return'] },
  ];
  const search = query.trim().toLocaleLowerCase();
  const matches = (value: string) => value.toLocaleLowerCase().includes(search);
  const choices: Choice[] = tab === 'existing'
    ? Object.keys(owner.nodes).filter(id => id !== sourceId && owner.nodes[id].type !== 'start' && (!openPort || !targetsFor(openPort).includes(id)) && (matches(id) || matches(nodeTitle(id)))).map(id => ({ id }))
    : (tab === 'tools' ? [{ label: tx('workflows:next-step.group-tools'), kinds: ['tool' as const] }] : groups)
      .flatMap(group => group.kinds.filter(kind => (matches(kind) || matches(tx(`workflows:kind.${kind}`)))).map(kind => ({ kind, group: group.label })));

  function nodeTitle(id: string) {
    return nodePresentation(document, block, id)?.title || id;
  }
  function open(port: Port, element: HTMLButtonElement) {
    if (openPort === port) { close(true); return; }
    trigger.current = element;
    refs.setReference(element);
    setTab('nodes'); setQuery(''); setActive(0); setOpenPort(port);
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
      const name = tx(`workflows:kind.${choice.kind}`);
      const count = Object.values(owner.nodes).filter(node => node.type === choice.kind).length;
      added = id;
      return setNodePresentation(next, block, id, { position, title: count ? `${name} ${count + 1}` : name, description: tx(`workflows:kind-about.${choice.kind}`) });
    });
    close();
    if (added) onSelect({ kind: 'node', id: added, path: block });
  }

  return <section className={css.nextSection} aria-label={tx('workflows:inspector.next-step')}>
    <h3 className={css.nextHeading}>{tx('workflows:inspector.next-step')}</h3>
    <p className={css.nextHint}>{tx('workflows:next-step.hint')}</p>
    <div className={css.nextFlow}>
      <span className={clsx(css.nextSource, sourceNode?.type === 'start' ? css.startIcon : css[`kind_${sourceNode!.type}`])}>
        {sourceNode?.type === 'start' ? <IconEntry size={18} /> : <KindIcon kind={sourceNode!.type} size={18} />}
      </span>
      <span className={css.nextConnector} aria-hidden="true" />
      <div className={css.nextPorts}>
        {ports.map(port => {
          const targets = targetsFor(port);
          return <div key={port} className={css.nextPort}>
            {ports.length > 1 && <span className={css.nextPortLabel}>{portLabel(port)}</span>}
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
      onKeyDown={event => {
        if (event.nativeEvent.isComposing) return;
        if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(true); }
        else if (event.target === input.current && event.key === 'ArrowDown') { event.preventDefault(); setActive(index => Math.min(index + 1, Math.max(0, choices.length - 1))); }
        else if (event.target === input.current && event.key === 'ArrowUp') { event.preventDefault(); setActive(index => Math.max(index - 1, 0)); }
        else if (event.target === input.current && event.key === 'Enter' && choices[active]) { event.preventDefault(); choose(choices[active]); }
      }}>
      <div className={css.nextTabs} role="tablist" aria-label={tx('workflows:next-step.picker')}>
        {(['nodes', 'tools', 'existing'] as const).map(value => <button key={value} type="button" role="tab" aria-selected={tab === value}
          className={clsx(css.nextTab, tab === value && css.nextTabActive)} onClick={() => { setTab(value); setActive(0); input.current?.focus(); }}>{tx(`workflows:next-step.tab-${value}`)}</button>)}
      </div>
      <label className={css.nextSearch}><IconSearchOutline16 /><input ref={input} value={query} placeholder={tx('workflows:next-step.search')} aria-label={tx('workflows:next-step.search')}
        aria-controls="workflow-next-choices" aria-activedescendant={choices[active] ? `workflow-next-choice-${active}` : undefined}
        onChange={event => { setQuery(event.target.value); setActive(0); }} /></label>
      <div className={css.nextChoices} id="workflow-next-choices" role="listbox" aria-label={tx('workflows:next-step.picker')}>
        {choices.map((choice, index) => <div key={'id' in choice ? choice.id : choice.kind}>
          {'group' in choice && (!choices[index - 1] || !('group' in choices[index - 1]) || (choices[index - 1] as { group: string }).group !== choice.group) && <p className={css.nextGroup}>{choice.group}</p>}
          <button id={`workflow-next-choice-${index}`} data-node-id={'id' in choice ? choice.id : undefined} type="button" role="option" aria-selected={active === index}
            className={clsx(css.nextChoice, active === index && css.nextChoiceActive)} onMouseEnter={() => setActive(index)} onClick={() => choose(choice)}>
            {'kind' in choice ? <span className={clsx(css.nextChoiceIcon, css[`kind_${choice.kind}`])}><KindIcon kind={choice.kind} size={15} /></span>
              : <span className={clsx(css.nextChoiceIcon, css[`kind_${owner.nodes[choice.id].type}`])}><KindIcon kind={owner.nodes[choice.id].type} size={15} /></span>}
            <span>{'kind' in choice ? tx(`workflows:kind.${choice.kind}`) : nodeTitle(choice.id)}</span>
          </button>
        </div>)}
        {!choices.length && <p className={css.nextEmpty}>{tx('workflows:palette.none')}</p>}
      </div>
    </div>, window.document.body)}
  </section>;
}
