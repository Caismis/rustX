import { useRef, useState, type ReactNode, type RefObject } from 'react';
import clsx from 'clsx';
import type { WorkflowToolChoice, WorkflowToolSource } from '../../../../../../protocol/app-server/v62';
import { useTranslation } from '../../../../locale/react';
import type { Translate } from '../../../../locale/translation';
import { IconChevronRightOutline14, IconSearchOutline16 } from '../../../../presentation/primitives/icons';
import { replaceNode } from '../../document/commands';
import { blockAt, sameDocument, type BlockPath, type NodeKind, type WorkflowDocument } from '../../document/model';
import { KindIcon, SourceIcon, ToolIcon } from '../icons';
import { useToolCatalog } from './toolCatalog';
import css from './Editor.module.css';

/** A node kind, or a Tool node for one builtin Tool or one Tool of an MCP
 * server or Managed Python package, as Dify's Tools tab adds a node per tool. */
export type NodeChoice = { kind: Exclude<NodeKind, 'tool'> } | { kind: 'tool'; tool: WorkflowToolChoice; source?: string };
export type Choice = NodeChoice | { id: string };
type Tab = 'nodes' | 'tools' | 'existing';
interface Item { key: string; label: string; about: string; icon: ReactNode; tone: string; choice: Choice }
/** One row of a tab: a heading, a source provider that folds its Tools, a
 * note, or a choice (`index` among the tab's choices). */
type Row = { kind: 'heading'; key: string; label: string } | { kind: 'note'; key: string; label: string; retry?: () => void }
  | { kind: 'provider'; key: string; source: WorkflowToolSource; open: boolean; count: number } | { kind: 'option'; index: number; item: Item; nested?: boolean };

/** As Dify's block classifications: the default group has no heading. */
const GROUPS: readonly { group?: 'logic' | 'transform'; kinds: readonly Exclude<NodeKind, 'tool'>[] }[] = [
  { kinds: ['agent', 'model', 'return'] },
  { group: 'logic', kinds: ['branch', 'iteration', 'loop', 'loop_end', 'review'] },
  { group: 'transform', kinds: ['assigner'] },
];

/** A Tool's description up to its first sentence, as a node description. */
export function toolSummary(description: string): string {
  const line = description.split('\n')[0].trim(), end = line.search(/\.(\s|$)/);
  const sentence = end < 0 ? line : line.slice(0, end + 1);
  return sentence.length > 200 ? `${sentence.slice(0, 199)}…` : sentence;
}

/** A source's name as Dify names a provider: a package without its prefix. */
export const sourceName = (source: string) => source.replace(/^python:/, '');

/** The node a choice adds in place of its kind's template (added as `id`),
 * titled by its kind or Tool and numbered after the first of those. A Tool
 * node's result is the one its Tool declares. */
export function chosenNode(tx: Translate, document: WorkflowDocument, block: BlockPath, id: string, choice: NodeChoice): { document: WorkflowDocument; title: string; description: string } {
  const others = Object.entries(blockAt(document, block)!.nodes).filter(([key]) => key !== id).map(([, node]) => node);
  const numbered = (name: string, same: number) => same ? `${name} ${same + 1}` : name;
  if (choice.kind !== 'tool') return { document, title: numbered(tx(`workflows:kind.${choice.kind}`), others.filter(node => node.type === choice.kind).length), description: tx(`workflows:kind-about.${choice.kind}`) };
  const { tool, source } = choice;
  const selector = source === undefined ? { origin: 'builtin' as const, name: tool.name } : { origin: 'source' as const, source_id: source, name: tool.name };
  return {
    document: replaceNode(document, block, id, { type: 'tool', selector, arguments: { type: 'object', fields: {} }, result: tool.result }),
    title: numbered(tool.name, others.filter(node => node.type === 'tool' && sameDocument(node.selector, selector)).length),
    description: toolSummary(tool.description),
  };
}

/** Dify's block selector: Nodes and Tools tabs (and, for a next step, the
 * existing nodes), a search per tab, classified rows and a preview card.
 * The Tools tab lists the builtin Tools, then each MCP server and Managed
 * Python package of this scope with its Tools, listed on first open. */
export function NodeChooser({ tools, omit = [], existing, onChoose, input, preview = 'right' }: {
  tools: readonly WorkflowToolChoice[]; omit?: readonly NodeKind[]; existing?: readonly { id: string; title: string; kind: NodeKind }[];
  onChoose: (choice: Choice) => void; input?: RefObject<HTMLInputElement | null>; preview?: 'left' | 'right';
}) {
  const tx = useTranslation(), catalog = useToolCatalog(), list = useRef<HTMLDivElement>(null), own = useRef<HTMLInputElement>(null), field = input ?? own;
  const [tab, setTab] = useState<Tab>('nodes'), [queries, setQueries] = useState<Record<Tab, string>>({ nodes: '', tools: '', existing: '' });
  const [active, setActive] = useState(0), [shown, setShown] = useState<{ index: number; top: number }>(), [opened, setOpened] = useState<ReadonlySet<string>>(new Set());
  const search = queries[tab].trim().toLocaleLowerCase(), matches = (...values: string[]) => values.some(value => value.toLocaleLowerCase().includes(search));
  const rows: Row[] = [], items: Item[] = [];
  const option = (item: Item, nested?: boolean) => { rows.push({ kind: 'option', index: items.length, item, nested }); items.push(item); };
  const toolItem = (tool: WorkflowToolChoice, source?: WorkflowToolSource): Item => ({
    key: `tool:${source?.source_id ?? ''}:${tool.name}`, label: tool.name, about: toolSummary(tool.description),
    icon: source ? <SourceIcon kind={source.kind} size={15} /> : <ToolIcon name={tool.name} size={15} />, tone: css.kind_tool, choice: { kind: 'tool', tool, source: source?.source_id },
  });
  if (tab === 'existing') for (const node of existing ?? []) {
    if (matches(node.id, node.title)) option({ key: node.id, label: node.title, about: node.id, icon: <KindIcon kind={node.kind} size={15} />, tone: css[`kind_${node.kind}`], choice: { id: node.id } });
  } else if (tab === 'nodes') for (const { group, kinds } of GROUPS) {
    const shownKinds = kinds.filter(kind => !omit.includes(kind) && matches(kind, tx(`workflows:kind.${kind}`)));
    if (group && shownKinds.length) rows.push({ kind: 'heading', key: group, label: tx(`workflows:palette.group-${group}`) });
    for (const kind of shownKinds) option({ key: kind, label: tx(`workflows:kind.${kind}`), about: tx(`workflows:kind-about.${kind}`), icon: <KindIcon kind={kind} size={15} />, tone: css[`kind_${kind}`], choice: { kind } });
  } else if (!omit.includes('tool')) {
    const builtin = tools.filter(tool => matches(tool.name, tool.description));
    if (builtin.length) rows.push({ kind: 'heading', key: 'builtin', label: tx('workflows:palette.group-builtin') });
    for (const tool of builtin) option(toolItem(tool));
    if (catalog.state.status === 'ready') {
      const sources = catalog.state.sources.map(source => ({ source, tools: source.tools.filter(tool => matches(tool.name, tool.description, source.source_id)) }))
        .filter(({ source, tools }) => tools.length || !search || matches(source.source_id));
      if (sources.length) rows.push({ kind: 'heading', key: 'sources', label: tx('workflows:palette.group-sources') });
      for (const { source, tools } of sources) {
        // As Dify: a search opens every provider with a matching Tool.
        const open = !!search || opened.has(source.source_id);
        rows.push({ kind: 'provider', key: source.source_id, source, open, count: source.tools.length });
        if (open) for (const tool of tools) option(toolItem(tool, source), true);
      }
    } else rows.push(catalog.state.status === 'failed'
      ? { kind: 'note', key: 'catalog', label: tx('workflows:palette.sources-failed'), retry: catalog.load }
      : { kind: 'note', key: 'catalog', label: tx('workflows:palette.sources-loading') });
  }
  const tabs: Tab[] = existing ? ['nodes', 'tools', 'existing'] : ['nodes', 'tools'];
  const choose = (value: Tab) => { setTab(value); setActive(0); setShown(undefined); if (value === 'tools' && catalog.state.status === 'idle') catalog.load(); field.current?.focus(); };
  const toggle = (source: string) => setOpened(current => { const next = new Set(current); if (!next.delete(source)) next.add(source); return next; });
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
        onClick={() => choose(value)}>{tx(`workflows:palette.tab-${value}`)}</button>)}
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
      {rows.map(row => row.kind === 'heading' ? <p key={`heading:${row.key}`} className={css.chooserGroup}>{row.label}</p>
        : row.kind === 'note' ? <p key={`note:${row.key}`} className={css.chooserNote} role="status">{row.label}
          {row.retry && <button type="button" className={css.chooserRetry} onClick={row.retry}>{tx('workflows:palette.retry')}</button>}</p>
        : row.kind === 'provider' ? <button key={`provider:${row.key}`} type="button" className={css.chooserProvider} aria-expanded={row.open} disabled={row.source.state !== 'listed'} onClick={() => toggle(row.key)}>
          <span className={clsx(css.chooserIcon, css.kind_tool)}><SourceIcon kind={row.source.kind} size={15} /></span>
          <span className={css.chooserLabel}>{sourceName(row.source.source_id)}</span>
          <span className={css.chooserBadge}>{tx(`workflows:palette.scope-${row.source.scope}`)}</span>
          <small className={css.chooserCount}>{row.source.state === 'listed' ? row.count : tx(`workflows:palette.source-${row.source.state}`)}</small>
          {row.source.state === 'listed' && <IconChevronRightOutline14 className={css.chooserChevron} />}
        </button>
        : <button key={row.item.key} id={`workflow-choice-${row.index}`} data-choice-index={row.index} data-node-id={'id' in row.item.choice ? row.item.choice.id : undefined} type="button" role="option" aria-selected={active === row.index} aria-description={row.item.about}
          className={clsx(css.chooserOption, row.nested && css.chooserNested, active === row.index && css.chooserActive)} onMouseEnter={() => show(row.index)} onClick={() => onChoose(row.item.choice)}>
          <span className={clsx(css.chooserIcon, row.item.tone)}>{row.item.icon}</span><span className={css.chooserLabel}>{row.item.label}</span>
        </button>)}
      {!rows.length && <p className={css.chooserEmpty}>{tx('workflows:palette.none')}</p>}
    </div>
    {current && <div className={clsx(css.chooserPreview, preview === 'left' && css.chooserPreviewLeft)} style={{ top: shown.top }} aria-hidden>
      <span className={clsx(css.chooserIcon, css.chooserPreviewIcon, current.tone)}>{current.icon}</span>
      <b>{current.label}</b>
      <p>{current.about}</p>
    </div>}
  </div>;
}
