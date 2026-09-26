import { message, displayText, searchVocabulary, type DisplayText } from '../../locale/translation';
import { useTranslation, useNotice } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Rewritten from ui-commands/PopupSelectView.tsx; see PROVENANCE.md. */
import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { CompletedResponseView, UserInputBlock, SessionSnapshot } from '../../../../protocol/app-server/v23';
import type { AppServerClient } from '../../client/app-server';
import { activeAttempt, lineageSwitchSafe } from '../../bindings/projection';
import { Modal } from '../../presentation/primitives/Modal';
import { Button } from '../../presentation/primitives/Button';
import { CommandSession, type HistoricalSelection, type HistoryAction } from './native';
import type { CommandId } from './registry';
import css from './Commands.module.css';

type Choice = { kind: 'model'; model: string; profile?: string } | { kind: 'history'; action: HistoryAction; selection: HistoricalSelection } | { kind: 'node'; nodeId: string; conversationId: string };
interface Row { id: string; label: DisplayText; detail?: DisplayText; choice: Choice }
export interface CommandRequest { id: Exclude<CommandId, 'new'> | 'retry' | 'tree'; messageId?: string; response?: CompletedResponseView }
export function CommandPanel({ request, client, sessionId, current, close, succeeded, opened }: {
  request: CommandRequest; client: AppServerClient; sessionId: string; current: () => boolean;
  close: () => void; succeeded: () => void; opened: (result: { session: SessionSnapshot; content: UserInputBlock[] }) => void;
}) {
  const tx = useTranslation();
  const [rows, setRows] = useState<Row[]>([]), [query, setQuery] = useState(''), [active, setActive] = useState(0);
  const [busy, setBusy] = useState(true), [error, setError] = useNotice(), [detail, setDetail] = useNotice();
  const [stopped, setStopped] = useState(false);
  const [next, setNext] = useState<number | null | undefined>();
  const alive = useRef(true), selecting = useRef(false);
  const options = useRef<HTMLDivElement>(null);
  const [scope] = useState(() => new CommandSession(client, sessionId, () => alive.current && current()));
  const view = useSyncExternalStore(client.subscribe, client.getSnapshot).views[sessionId];
  const blocked = request.id === 'compact' ? activeAttempt(view?.snapshot)
    : ['branch', 'retry', 'tree'].includes(request.id) && !lineageSwitchSafe(view);
  const valid = () => alive.current && current();
  const historical = request.id === 'fork' || request.id === 'branch' || request.id === 'retry';
  const loadBoundaries = async (offset: number) => {
    const action = request.id;
    if (action !== 'fork' && action !== 'branch' && action !== 'retry') { setError(message('commands:copy.not-a-historical-command')); return; }
    setRows([]);
    if (request.response) {
      const selection = await scope.responseSelection(request.response);
      if (!valid()) return;
      setRows([{ id: request.response.closing_message_id, label: action === 'retry' ? message('commands:command-panel.replay-the-original-input-once') : message('commands:command-panel.continue-after-this-response'), choice: { kind: 'history', action, selection } }]);
      setNext(null); setActive(0); return;
    }
    const page = await scope.boundaries(offset);
    if (!valid()) return;
    setRows(page.selections.filter(selection => !request.messageId || selection.boundary.message.id === request.messageId).map(selection => ({ id: selection.boundary.message.id,
      label: selection.boundary.message.content.map(block => block.type === 'text' ? block.text : block.type === 'uploaded_file' ? block.name : `[${block.type}]`).join(' ').slice(0, 240),

      choice: { kind: 'history', action, selection } })));
    setNext(page.nextOffset); setActive(0);
    if (request.messageId && !page.selections.some(item => item.boundary.message.id === request.messageId)) {
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
        case 'model': {
          const result = await scope.models(); if (!valid()) return;
          setDetail(message('commands:copy.current-value-choosing-a-model-uses-its-native-defaults', { p0: result.current.configured.model }));
          setRows((result.catalog.models ?? []).flatMap(model => [
            { id: model.model, label: model.model, detail: message('commands:copy.native-defaultvalue', { p0: model.defaultReasoningProfile ? ` · ${model.defaultReasoningProfile}` : '' }), choice: { kind: 'model' as const, model: model.model } },
            ...(model.reasoningProfiles ?? []).map(profile => ({ id: `${model.model}:${profile.id}`, label: message('commands:command-panel.value-value', { p0: model.model, p1: profile.id }), detail: message('commands:copy.reasoning-profile'), choice: { kind: 'model' as const, model: model.model, profile: profile.id } })),
          ])); break;
        }
        case 'fork': case 'branch': case 'retry':
          setDetail(request.response && request.id !== 'retry' ? message('commands:copy.the-new-lineage-includes-this-completed-response-and-opens-with-an-empty-composer') : request.id === 'fork' ? message('commands:copy.independent-session-choose-the-exact-user-boundary-its-prompt-returns-to-the-composer') : request.id === 'retry' ? message('commands:copy.create-a-native-branch-switch-the-idle-session-to-it-and-execute-the-selected-prompt-once-') : message('commands:copy.create-a-native-branch-and-switch-the-idle-session-to-it-the-selected-prompt-returns-to-th'));
          await loadBoundaries(0); break;
        case 'tools': { const result = await scope.tools(); if (valid()) { setDetail(JSON.stringify(result, null, 2)); succeeded(); } break; }
        case 'compact': setDetail(message('commands:copy.compact-this-session-through-the-native-context-owner')); break;
        case 'tree': setDetail(message('commands:copy.native-session-lineage-opening-another-node-switches-the-idle-resident-runtime-the-origina')); await loadTree(0); break;
        case 'goal': setError(message('commands:copy.goal-controls-are-in-the-goal-dock')); break;
        default: { const exhaustive: never = request.id; throw new Error(String(exhaustive)); }
      }
    };
    void load().catch(cause => { if (valid()) setError(String(cause)); }).finally(() => { if (valid()) setBusy(false); });
    return () => { alive.current = false; };
  }, []);
  const choose = async (choice: Choice) => {
    if (selecting.current || busy || stopped || blocked || !valid()) return;
    selecting.current = true; setBusy(true); setError('');
    try {
      switch (choice.kind) {
        case 'model': await scope.setModel(choice.model, choice.profile); if (valid() && scope.current()) { succeeded(); close(); } break;
        case 'node': if (await scope.openNode(choice.nodeId, choice.conversationId) && valid()) close(); break;
        case 'history': {
          const result = await scope.transition(choice.action, choice.selection);
          if (valid() && result) opened(result); break;
        }
        default: { const exhaustive: never = choice; throw new Error(String(exhaustive)); }
      }
    } catch (cause) { if (valid()) { setError(message('commands:copy.value-close-and-reread-authoritative-state-before-another-mutation', { p0: String(cause) })); setStopped(true); } }
    finally { selecting.current = false; if (valid()) setBusy(false); }
  };
  const perform = async () => {
    if (selecting.current || busy || stopped || blocked || !valid()) return;
    selecting.current = true; setBusy(true); setError('');
    try {
      if (request.id === 'compact') { await scope.compact(); if (valid() && scope.current()) { succeeded(); close(); } }
    } catch (cause) { if (valid()) { setError(message('commands:copy.value-close-and-reread-authoritative-state-before-another-mutation', { p0: String(cause) })); setStopped(true); } }
    finally { selecting.current = false; if (valid()) setBusy(false); }
  };
  const filtered = rows.filter(row => `${searchVocabulary(row.label)} ${searchVocabulary(row.detail ?? '')}`.toLowerCase().includes(query.toLowerCase()));
  useEffect(() => { options.current?.querySelector('[aria-selected="true"]')?.scrollIntoView?.({ block: 'nearest' }); }, [active, query]);
  const stale = !scope.current();
  return <Modal closeLabel={tx('commands:command-panel.close-dialog')} open title={request.id === 'tree' ? tx('commands:command-panel.session-tree') : request.id === 'retry' ? tx('commands:command-panel.retry-regenerate') : tx('commands:command-panel.value', { p0: request.id })} onClose={close}>
    <p style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{detail}</p>
    {error && <p role="alert">{error}</p>}
    {blocked && <p role="status">{tx('commands:command-panel.waiting-for-native-execution-and-accepted-inbound-to-settle')}</p>}
    {!busy && stale && <p role="status">{tx('commands:command-panel.attachment-changed-close-and-reopen-to-read-current-native-state')}</p>}
    {busy && <p role="status">{tx('commands:command-panel.waiting-for-native-acknowledgement')}</p>}
    {!busy && !error && !rows.length && (historical || request.id === 'tree' || request.id === 'model') && <p role="status">{tx('commands:command-panel.no-native-choices-available')}</p>}
    {request.id === 'compact' && <Button disabled={busy || stopped || stale || blocked} onClick={() => void perform()}>{tx('commands:command-panel.compact-context')}</Button>}
    {!!rows.length && <><input autoFocus className={css.search} aria-label={tx('commands:command-panel.filter-options')} value={query} disabled={busy}
      onChange={event => { setQuery(event.target.value); setActive(0); }} aria-controls="command-options" aria-activedescendant={filtered[active] ? `choice-${active}` : undefined}
      onKeyDown={event => {
        if ((event.key === 'ArrowDown' || event.key === 'ArrowUp') && filtered.length) { event.preventDefault(); setActive(index => (index + (event.key === 'ArrowDown' ? 1 : filtered.length - 1)) % filtered.length); }
        if (event.key === 'Enter' && filtered[active] && !event.nativeEvent.isComposing) { event.preventDefault(); void choose(filtered[active].choice); }
      }} />
      <div ref={options} className={css.options} id="command-options" role="listbox" aria-label={historical ? tx('commands:command-panel.historical-boundaries') : tx('commands:command-panel.native-choices')}>{filtered.map((row, index) => <button type="button" role="option" data-choice-id={row.id} id={`choice-${index}`} aria-selected={active === index} disabled={busy || stopped || stale || blocked} key={row.id} className={css.row} onClick={() => void choose(row.choice)}>
        <span>{displayText(tx, row.label)}</span><small>{displayText(tx, row.detail ?? '')}</small>
      </button>)}</div></>}
    {(historical || request.id === 'tree') && next != null && !request.messageId && <Button disabled={busy} onClick={() => { setBusy(true); void (request.id === 'tree' ? loadTree(next) : loadBoundaries(next)).catch(cause => { if (valid()) setError(String(cause)); }).finally(() => { if (valid()) setBusy(false); }); }}>{tx('commands:command-panel.more')}{' '}{historical ? tx('commands:command-panel.boundaries') : tx('commands:command-panel.nodes')}</Button>}
  </Modal>;
}
