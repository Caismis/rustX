import { message, displayText, searchVocabulary, type DisplayText } from '../../locale/translation';
import { useTranslation, useNotice } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Rewritten from ui-commands/PopupSelectView.tsx; see PROVENANCE.md. */
import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { AppServerClient } from '../../client/app-server';
import type { CheckpointPage } from '../../../../protocol/app-server/v57';
import { lineageSwitchSafe } from '../../bindings/projection';
import { Modal } from '../../presentation/primitives/Modal';
import { Button } from '../../presentation/primitives/Button';
import { CommandSession, type HistoricalSelection, type HistoryAction } from './native';
import type { CommandId } from './registry';
import css from './Commands.module.css';

export type Choice = { kind: 'history'; action: HistoryAction; selection: HistoricalSelection } | { kind: 'node'; nodeId: string; conversationId: string };
interface Row { id: string; label: DisplayText; detail?: DisplayText; choice: Choice }
export interface CommandRequest { id: Exclude<CommandId, 'new' | 'compact' | 'model'> | 'tree'; messageId?: string }
export function CommandPanel({ request, client, sessionId, current, close, succeeded, navigate }: {
  request: CommandRequest; client: AppServerClient; sessionId: string; current: () => boolean;
  close: () => void; succeeded: () => void; navigate: (choice: Choice) => void;
}) {
  const tx = useTranslation();
  const [rows, setRows] = useState<Row[]>([]), [query, setQuery] = useState(''), [active, setActive] = useState(0);
  const [busy, setBusy] = useState(true), [error, setError] = useNotice(), [detail, setDetail] = useNotice();
  const [next, setNext] = useState<number | null | undefined>();
  const [toolsPage, setToolsPage] = useState<CheckpointPage>();
  const alive = useRef(true), selecting = useRef(false);
  const options = useRef<HTMLDivElement>(null);
  const [scope] = useState(() => new CommandSession(client, sessionId, () => alive.current && current()));
  const view = useSyncExternalStore(client.subscribe, client.getSnapshot).views[sessionId];
  const blocked = ['branch', 'tree'].includes(request.id) && !lineageSwitchSafe(view);
  const valid = () => alive.current && current();
  const historical = request.id === 'fork' || request.id === 'branch';
  const loadBoundaries = async (offset: number) => {
    const action = request.id;
    if (action !== 'fork' && action !== 'branch') { setError(message('commands:copy.not-a-historical-command')); return; }
    setRows([]);
    const page = await scope.boundaries(offset);
    if (!valid()) return;
    const matches = (selection: typeof page.selections[number]) => !request.messageId || selection.boundary.locator.type === 'by_message_id' && selection.boundary.locator.message_id === request.messageId;
    setRows(page.selections.filter(matches).map(selection => ({ id: JSON.stringify(selection.boundary.locator),
      label: selection.boundary.message ? selection.boundary.message.content.map(block => block.type === 'text' ? block.text : block.type === 'uploaded_file' ? block.name : `[${block.type}]`).join(' ').slice(0, 240) : message('common:activity.loading'),

      choice: { kind: 'history', action, selection } })));
    setNext(page.nextOffset); setActive(0);
    if (request.messageId && !page.selections.some(matches)) {
      if (page.nextOffset != null) return loadBoundaries(page.nextOffset);
      setError(message('commands:copy.this-message-is-not-an-available-native-user-boundary-no-mutation-was-sent'));
    }
  };
  const loadTree = async (offset: number) => {
    setRows([]);
    const tree = await scope.tree(offset); if (!valid()) return;
    setRows(tree.nodes.map(node => ({ id: node.id, label: node.id, detail: message('commands:tree.node-detail', { conversation: node.conversation_id, origin: message(`commands:tree.origin.${node.origin.type}`), parent: node.parent ? message('commands:copy.parent-value', { p0: node.parent }) : '' }), choice: { kind: 'node', nodeId: node.id, conversationId: node.conversation_id } })));
    setNext(tree.next_offset); setActive(0);
  };
  useEffect(() => {
    alive.current = true;
    const load = async () => {
      switch (request.id) {
        case 'fork': case 'branch':
          setDetail(request.id === 'fork' ? message('commands:copy.independent-session-choose-the-exact-user-boundary-its-prompt-returns-to-the-composer') : message('commands:copy.create-a-native-branch-and-switch-the-idle-session-to-it-the-selected-prompt-returns-to-th'));
          await loadBoundaries(0); break;
        case 'tools': { const page = await scope.tools(); if (valid()) { setToolsPage(page); succeeded(); } break; }
        case 'tree': setDetail(message('commands:copy.native-session-lineage-opening-another-node-switches-the-idle-resident-runtime-the-origina')); await loadTree(0); break;
        case 'goal': setError(message('commands:copy.goal-controls-are-in-the-goal-dock')); break;
        default: { const exhaustive: never = request.id; throw new Error(String(exhaustive)); }
      }
    };
    void load().catch(cause => { if (valid()) setError(String(cause)); }).finally(() => { if (valid()) setBusy(false); });
    return () => { alive.current = false; };
  }, []);
  const choose = async (choice: Choice) => {
    if (selecting.current || busy || blocked || !valid()) return;
    selecting.current = true;
    navigate(choice);
  };
  const filtered = rows.filter(row => `${searchVocabulary(row.label)} ${searchVocabulary(row.detail ?? '')}`.toLowerCase().includes(query.toLowerCase()));
  useEffect(() => { options.current?.querySelector('[aria-selected="true"]')?.scrollIntoView?.({ block: 'nearest' }); }, [active, query]);
  const stale = !scope.current();
  return <Modal closeLabel={tx('commands:command-panel.close-dialog')} open title={request.id === 'tree' ? tx('commands:command-panel.session-tree') : tx('commands:command-panel.value', { p0: request.id })} onClose={close}>
    <p style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{detail}</p>
    {toolsPage && <>
      {toolsPage.items.map(item => item.value != null ? <pre key={item.offset}>{JSON.stringify(item.value, null, 2)}</pre> : null)}
    </>}
    {error && <p role="alert">{error}</p>}
    {blocked && <p role="status">{tx('commands:command-panel.waiting-for-native-execution-and-accepted-inbound-to-settle')}</p>}
    {!busy && stale && <p role="status">{tx('commands:command-panel.attachment-changed-close-and-reopen-to-read-current-native-state')}</p>}
    {busy && <p role="status">{tx('commands:command-panel.waiting-for-native-acknowledgement')}</p>}
    {!busy && !error && !rows.length && (historical || request.id === 'tree') && <p role="status">{tx('commands:command-panel.no-native-choices-available')}</p>}
    {!!rows.length && <><input autoFocus className={css.search} aria-label={tx('commands:command-panel.filter-options')} value={query} disabled={busy}
      onChange={event => { setQuery(event.target.value); setActive(0); }} aria-controls="command-options" aria-activedescendant={filtered[active] ? `choice-${active}` : undefined}
      onKeyDown={event => {
        if ((event.key === 'ArrowDown' || event.key === 'ArrowUp') && filtered.length) { event.preventDefault(); setActive(index => (index + (event.key === 'ArrowDown' ? 1 : filtered.length - 1)) % filtered.length); }
        if (event.key === 'Enter' && filtered[active] && !event.nativeEvent.isComposing) { event.preventDefault(); void choose(filtered[active].choice); }
      }} />
      <div ref={options} className={css.options} id="command-options" role="listbox" aria-label={historical ? tx('commands:command-panel.historical-boundaries') : tx('commands:command-panel.native-choices')}>{filtered.map((row, index) => <button type="button" role="option" data-choice-id={row.id} id={`choice-${index}`} aria-selected={active === index} disabled={busy || stale || blocked} key={row.id} className={css.row} onClick={() => void choose(row.choice)}>
        <span>{displayText(tx, row.label)}</span><small>{displayText(tx, row.detail ?? '')}</small>
      </button>)}</div></>}
    {(historical || request.id === 'tree') && next != null && !request.messageId && <Button disabled={busy} onClick={() => { setBusy(true); void (request.id === 'tree' ? loadTree(next) : loadBoundaries(next)).catch(cause => { if (valid()) setError(String(cause)); }).finally(() => { if (valid()) setBusy(false); }); }}>{tx('commands:command-panel.more')}{' '}{historical ? tx('commands:command-panel.boundaries') : tx('commands:command-panel.nodes')}</Button>}
  </Modal>;
}
