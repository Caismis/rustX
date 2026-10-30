import { useRef, useState, type ReactNode, type RefObject } from 'react';
import clsx from 'clsx';
import type { WorkflowToolChoice } from '../../../../../../protocol/app-server/v62';
import { useTranslation } from '../../../../locale/react';
import type { Translate } from '../../../../locale/translation';
import { IconSearchOutline16 } from '../../../../presentation/primitives/icons';
import { replaceNode } from '../../document/commands';
import { blockAt, type BlockPath, type NodeKind, type WorkflowDocument } from '../../document/model';
import { KindIcon, ToolIcon } from '../icons';
import css from './Editor.module.css';

/** A node kind, or a Tool node for one builtin Tool or for a source Tool
 * still to be named, as Dify's Tools tab adds a node per tool. */
export type NodeChoice = { kind: Exclude<NodeKind, 'tool'> } | { kind: 'tool'; tool?: WorkflowToolChoice };
export type Choice = NodeChoice | { id: string };
type Tab = 'nodes' | 'tools' | 'existing';
interface Item { key: string; group?: string; label: string; about: string; icon: ReactNode; tone: string; choice: Choice }

/** As Dify's block classifications: the default group has no heading. */
const GROUPS: readonly { group?: 'logic'; kinds: readonly Exclude<NodeKind, 'tool'>[] }[] = [
  { kinds: ['agent', 'model', 'return'] },
  { group: 'logic', kinds: ['branch', 'review', 'loop'] },
];

/** A Tool's description up to its first sentence, as a node description. */
export function toolSummary(description: string): string {
  const line = description.split('\n')[0].trim(), end = line.search(/\.(\s|$)/);
  const sentence = end < 0 ? line : line.slice(0, end + 1);
  return sentence.length > 200 ? `${sentence.slice(0, 199)}…` : sentence;
}

/** The node a choice adds in place of its kind's template (added as `id`),
 * titled by its kind or Tool and numbered after the first of those. */
export function chosenNode(tx: Translate, document: WorkflowDocument, block: BlockPath, id: string, choice: NodeChoice): { document: WorkflowDocument; title: string; description: string } {
  const others = Object.entries(blockAt(document, block)!.nodes).filter(([key]) => key !== id).map(([, node]) => node);
  const numbered = (name: string, same: number) => same ? `${name} ${same + 1}` : name;
  if (choice.kind !== 'tool') return { document, title: numbered(tx(`workflows:kind.${choice.kind}`), others.filter(node => node.type === choice.kind).length), description: tx(`workflows:kind-about.${choice.kind}`) };
  const tool = choice.tool;
  return {
    document: replaceNode(document, block, id, { type: 'tool', selector: tool ? { origin: 'builtin', name: tool.name } : { origin: 'source', source_id: '', name: '' }, arguments: { type: 'object', fields: {} }, result: { type: 'text', part: 0 } }),
    title: numbered(tool?.name ?? tx('workflows:kind.tool'), others.filter(node => node.type === 'tool' && (tool ? node.selector.origin === 'builtin' && node.selector.name === tool.name : node.selector.origin === 'source')).length),
    description: tool ? toolSummary(tool.description) : tx('workflows:kind-about.tool'),
  };
}

/** Dify's block selector: Nodes and Tools tabs (and, for a next step, the
 * existing nodes), a search per tab, classified rows and a preview card. */
export function NodeChooser({ tools, omit = [], existing, onChoose, input, preview = 'right' }: {
  tools: readonly WorkflowToolChoice[]; omit?: readonly NodeKind[]; existing?: readonly { id: string; title: string; kind: NodeKind }[];
  onChoose: (choice: Choice) => void; input?: RefObject<HTMLInputElement | null>; preview?: 'left' | 'right';
}) {
  const tx = useTranslation(), list = useRef<HTMLDivElement>(null), own = useRef<HTMLInputElement>(null), field = input ?? own;
  const [tab, setTab] = useState<Tab>('nodes'), [queries, setQueries] = useState<Record<Tab, string>>({ nodes: '', tools: '', existing: '' });
  const [active, setActive] = useState(0), [shown, setShown] = useState<{ index: number; top: number }>();
  const search = queries[tab].trim().toLocaleLowerCase(), matches = (...values: string[]) => values.some(value => value.toLocaleLowerCase().includes(search));
  const items: Item[] = tab === 'existing' ? (existing ?? []).filter(node => matches(node.id, node.title)).map(node => ({
    key: node.id, label: node.title, about: node.id, icon: <KindIcon kind={node.kind} size={15} />, tone: css[`kind_${node.kind}`], choice: { id: node.id },
  })) : tab === 'tools' ? [
    ...tools.filter(tool => !omit.includes('tool') && matches(tool.name, tool.description)).map(tool => ({
      key: `tool:${tool.name}`, group: tx('workflows:palette.group-builtin'), label: tool.name, about: toolSummary(tool.description),
      icon: <ToolIcon name={tool.name} size={15} />, tone: css.kind_tool, choice: { kind: 'tool', tool } as const,
    })),
    ...(!omit.includes('tool') && matches(tx('workflows:palette.source-tool'), tx('workflows:palette.source-tool-about')) ? [{
      key: 'tool', group: tx('workflows:palette.group-source'), label: tx('workflows:palette.source-tool'), about: tx('workflows:palette.source-tool-about'),
      icon: <KindIcon kind="tool" size={15} />, tone: css.kind_tool, choice: { kind: 'tool' } as const,
    }] : []),
  ] : GROUPS.flatMap(({ group, kinds }) => kinds.filter(kind => !omit.includes(kind) && matches(kind, tx(`workflows:kind.${kind}`))).map(kind => ({
    key: kind, group: group && tx(`workflows:palette.group-${group}`), label: tx(`workflows:kind.${kind}`), about: tx(`workflows:kind-about.${kind}`),
    icon: <KindIcon kind={kind} size={15} />, tone: css[`kind_${kind}`], choice: { kind } as const,
  })));
  const tabs: Tab[] = existing ? ['nodes', 'tools', 'existing'] : ['nodes', 'tools'];
  /** Show the card beside a row, as Dify's preview follows the hovered row.
   * The list is not positioned, so a row's offset is already the chooser's. */
  const show = (index: number) => {
    const row = list.current?.querySelector<HTMLElement>(`[data-choice-index="${index}"]`), box = list.current;
    setActive(index);
    if (row && box) setShown({ index, top: row.offsetTop - box.scrollTop });
  };
  const move = (index: number) => { show(index); list.current?.querySelector(`[data-choice-index="${index}"]`)?.scrollIntoView({ block: 'nearest' }); };
  const current = shown && items[shown.index];
  return <div className={css.chooser} onMouseLeave={() => setShown(undefined)}>
    <div className={css.chooserTabs} role="tablist">
      {tabs.map(value => <button key={value} type="button" role="tab" aria-selected={tab === value} className={clsx(css.chooserTab, tab === value && css.chooserTabActive)}
        onClick={() => { setTab(value); setActive(0); setShown(undefined); field.current?.focus(); }}>{tx(`workflows:palette.tab-${value}`)}</button>)}
    </div>
    <label className={css.chooserSearch}><IconSearchOutline16 /><input ref={field} value={queries[tab]} placeholder={tx(`workflows:palette.search-${tab}`)} aria-label={tx(`workflows:palette.search-${tab}`)}
      aria-controls="workflow-chooser" aria-activedescendant={items[active] ? `workflow-choice-${active}` : undefined}
      onChange={event => { const text = event.target.value; setQueries(value => ({ ...value, [tab]: text })); setActive(0); setShown(undefined); }}
      onKeyDown={event => {
        if (event.nativeEvent.isComposing) return;
        if (event.key === 'ArrowDown') { event.preventDefault(); move(Math.min(active + 1, items.length - 1)); }
        else if (event.key === 'ArrowUp') { event.preventDefault(); move(Math.max(active - 1, 0)); }
        else if (event.key === 'Enter' && items[active]) { event.preventDefault(); onChoose(items[active].choice); }
      }} /></label>
    <div ref={list} className={css.chooserList} id="workflow-chooser" role="listbox" aria-label={tx(`workflows:palette.tab-${tab}`)} onScroll={() => setShown(undefined)}>
      {items.map((item, index) => <div key={item.key}>
        {item.group && item.group !== items[index - 1]?.group && <p className={css.chooserGroup}>{item.group}</p>}
        <button id={`workflow-choice-${index}`} data-choice-index={index} data-node-id={'id' in item.choice ? item.choice.id : undefined} type="button" role="option" aria-selected={active === index} aria-description={item.about}
          className={clsx(css.chooserOption, active === index && css.chooserActive)} onMouseEnter={() => show(index)} onClick={() => onChoose(item.choice)}>
          <span className={clsx(css.chooserIcon, item.tone)}>{item.icon}</span><span className={css.chooserLabel}>{item.label}</span>
        </button>
      </div>)}
      {!items.length && <p className={css.chooserEmpty}>{tx('workflows:palette.none')}</p>}
    </div>
    {current && <div className={clsx(css.chooserPreview, preview === 'left' && css.chooserPreviewLeft)} style={{ top: shown.top }} aria-hidden>
      <span className={clsx(css.chooserIcon, css.chooserPreviewIcon, current.tone)}>{current.icon}</span>
      <b>{current.label}</b>
      <p>{current.about}</p>
    </div>}
  </div>;
}
