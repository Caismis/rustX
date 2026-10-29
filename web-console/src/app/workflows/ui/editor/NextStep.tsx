import { autoUpdate, flip, offset, shift, useFloating } from '@floating-ui/react-dom';
import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import clsx from 'clsx';
import { useTranslation } from '../../../../locale/react';
import { IconCloseOutline16, IconSearchOutline16 } from '../../../../presentation/primitives/icons';
import { addNode, clearEntry, connect, disconnect, freshIdentifier, insertEntry, insertNode, reconnect, setEntry } from '../../document/commands';
import { nodePresentation, setNodePresentation } from '../../document/geometry';
import { blockAt, edgesOf, nodeAt, sameBlockPath, NODE_KINDS, type BlockPath, type NodeKind, type WorkflowDocument } from '../../document/model';
import { edgePort, outputPorts, type ControlPort } from '../../document/ports';
import { IconEntry, IconPlus, KindIcon } from '../icons';
import { NODE_WIDTH, projectScene } from './scene';
import { viewId, type Selection } from './selection';
import type { Edit } from './panels/shared';
import css from './Editor.module.css';

type Port = ControlPort | 'entry';
type Tab = 'nodes' | 'tools' | 'existing';
type Choice = { kind: NodeKind; group: string } | { id: string };

/** The inspector's continuation uses the same native graph commands as the
 * canvas. A filled port inserts before its current target, preserving the path. */
export function NextStep({ document, block, source, edit, onSelect }: {
  document: WorkflowDocument; block: BlockPath; source?: string; edit: Edit; onSelect: (selection: Selection) => void;
}) {
  const tx = useTranslation(), owner = blockAt(document, block)!;
  const sourceNode = source ? nodeAt(owner, source) : undefined;
  const ports: readonly Port[] = sourceNode ? outputPorts(sourceNode.type) : ['entry'];
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

  const targetFor = (port: Port) => port === 'entry'
    ? (nodeAt(owner, owner.entry) ? owner.entry : undefined)
    : edgesOf(owner).find(edge => edge.from === source && edgePort(edge) === port)?.to;
  const portLabel = (port: Port) => port === 'entry' ? tx('workflows:inspector.next-step') : tx(`workflows:port.${port}`);
  const currentTarget = openPort && targetFor(openPort);
  const insertion = !!currentTarget;
  const availableKinds = NODE_KINDS.filter(kind => !insertion || (outputPorts(kind).length === 1 && outputPorts(kind)[0] === 'next'));
  const groups: { label: string; kinds: readonly NodeKind[] }[] = [
    { label: tx('workflows:next-step.group-ai'), kinds: ['agent', 'model'] },
    { label: tx('workflows:next-step.group-control'), kinds: ['branch', 'parallel', 'loop'] },
    { label: tx('workflows:next-step.group-result'), kinds: ['review', 'return'] },
  ];
  const search = query.trim().toLocaleLowerCase();
  const matches = (value: string) => value.toLocaleLowerCase().includes(search);
  const choices: Choice[] = tab === 'existing'
    ? Object.keys(owner.nodes).filter(id => id !== source && (matches(id) || matches(nodeTitle(id)))).map(id => ({ id }))
    : (tab === 'tools' ? [{ label: tx('workflows:next-step.group-tools'), kinds: ['tool' as const] }] : groups)
      .flatMap(group => group.kinds.filter(kind => availableKinds.includes(kind) && (matches(kind) || matches(tx(`workflows:kind.${kind}`)))).map(kind => ({ kind, group: group.label })));

  function nodeTitle(id: string) {
    return nodePresentation(document, block, id)?.title || id;
  }
  function open(port: Port, element: HTMLButtonElement) {
    if (openPort === port) { close(true); return; }
    trigger.current = element;
    refs.setReference(element);
    setTab('nodes'); setQuery(''); setActive(0); setOpenPort(port);
  }
  function remove(port: Port) {
    edit(current => {
      if (port === 'entry') return clearEntry(current, block);
      const index = edgesOf(blockAt(current, block)!).findIndex(edge => edge.from === source && edgePort(edge) === port);
      return index < 0 ? current : disconnect(current, block, [index]);
    });
  }
  function choose(choice: Choice) {
    if (!openPort) return;
    const port = openPort;
    if ('id' in choice) {
      edit(current => {
        if (port === 'entry') return setEntry(current, block, choice.id);
        const index = edgesOf(blockAt(current, block)!).findIndex(edge => edge.from === source && edgePort(edge) === port);
        return index < 0 ? connect(current, block, source!, port, choice.id) : reconnect(current, block, index, source!, port, choice.id);
      });
      close(); return;
    }
    let added = '';
    edit(current => {
      const owner = blockAt(current, block)!;
      const id = freshIdentifier(owner, choice.kind);
      const edges = edgesOf(owner), index = port === 'entry' ? -1 : edges.findIndex(edge => edge.from === source && edgePort(edge) === port);
      const target = port === 'entry' ? (nodeAt(owner, owner.entry) ? owner.entry : undefined) : edges[index]?.to;
      let next = port === 'entry'
        ? target ? insertEntry(current, block, choice.kind, id) : setEntry(addNode(current, block, choice.kind, id), block, id)
        : target ? insertNode(current, block, index, choice.kind, id) : addNode(current, block, choice.kind, id, { from: source!, port });
      const scene = projectScene(current).nodes;
      const origin = scene.find(node => node.id === viewId(block, source ? 'node' : 'start', source));
      const targetNode = target && scene.find(node => node.id === viewId(block, 'node', target));
      const position = { x: (origin?.position.x ?? 0) + (origin?.width ?? NODE_WIDTH) + 96,
        y: (origin?.position.y ?? 80) + (ports.length > 1 ? ports.indexOf(port) * 88 : 0) };
      if (targetNode && origin && targetNode.position.x > origin.position.x) {
        const dx = Math.max(0, position.x + NODE_WIDTH + 96 - targetNode.position.x);
        if (dx) for (const node of scene) if (node.data.view === 'node' && node.data.local !== source && sameBlockPath(node.data.path, block) && node.position.x >= targetNode.position.x - 1)
          next = setNodePresentation(next, block, node.data.local, { position: { x: node.position.x + dx, y: node.position.y } });
      } else {
        while (scene.some(node => node.data.view !== 'lane' && sameBlockPath(node.data.path, block) && position.x < node.position.x + (node.width ?? NODE_WIDTH)
          && position.x + NODE_WIDTH > node.position.x && position.y < node.position.y + (node.height ?? 128) + 24 && position.y + 152 > node.position.y))
          position.y += 176;
      }
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
      <span className={clsx(css.nextSource, sourceNode ? css[`kind_${sourceNode.type}`] : css.startIcon)}>
        {sourceNode ? <KindIcon kind={sourceNode.type} size={18} /> : <IconEntry size={18} />}
      </span>
      <span className={css.nextConnector} aria-hidden="true" />
      <div className={css.nextPorts}>
        {ports.map(port => {
          const target = targetFor(port), targetNode = target && nodeAt(owner, target);
          return <div key={port} className={css.nextPort}>
            {ports.length > 1 && <span className={css.nextPortLabel}>{portLabel(port)}</span>}
            {target && targetNode && <div className={css.nextConnected}>
              <button type="button" className={css.nextTarget} title={nodeTitle(target)} onClick={() => onSelect({ kind: 'node', id: target, path: block })}>
                <span className={clsx(css.nextTargetIcon, css[`kind_${targetNode.type}`])}><KindIcon kind={targetNode.type} size={15} /></span>
                <span>{nodeTitle(target)}</span>
              </button>
              <button type="button" className={css.nextRemove} aria-label={tx('workflows:next-step.disconnect', { p0: nodeTitle(target) })} title={tx('workflows:next-step.disconnect', { p0: nodeTitle(target) })} onClick={() => remove(port)}><IconCloseOutline16 /></button>
            </div>}
            <button type="button" className={css.nextAdd} aria-label={tx(target ? 'workflows:next-step.insert' : 'workflows:next-step.choose', { p0: ports.length > 1 ? portLabel(port) : tx('workflows:inspector.next-step') })}
              aria-haspopup="dialog" aria-expanded={openPort === port}
              onPointerDown={event => { if (event.button === 0) { event.preventDefault(); event.stopPropagation(); open(port, event.currentTarget); } }}
              onClick={event => { if (event.detail === 0) open(port, event.currentTarget); }}>
              <span className={css.nextAddIcon}><IconPlus size={13} /></span>
              {tx(target ? 'workflows:next-step.insert-label' : 'workflows:next-step.choose-label')}
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
