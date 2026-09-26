/* Copyright (c) 2026 DeepSeek. MIT. Rewritten from ui-commands/PopupSelectView.tsx; see PROVENANCE.md. */
import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { CompletedResponseView, UserInputBlock, SessionSnapshot } from '../../../../protocol/app-server/v25';
import type { AppServerClient } from '../../client/app-server';
import { activeAttempt, lineageSwitchSafe } from '../../bindings/projection';
import { Modal } from '../../presentation/primitives/Modal';
import { Button } from '../../presentation/primitives/Button';
import { CommandSession, type HistoricalSelection, type HistoryAction } from './native';
import type { CommandId } from './registry';
import css from './Commands.module.css';

type Choice = { kind: 'model'; model: string; profile?: string } | { kind: 'history'; action: HistoryAction; selection: HistoricalSelection } | { kind: 'node'; nodeId: string; conversationId: string };
interface Row { id: string; label: string; detail?: string; choice: Choice }
export interface CommandRequest { id: Exclude<CommandId, 'new'> | 'retry' | 'tree'; messageId?: string; response?: CompletedResponseView }
export function CommandPanel({ request, client, sessionId, current, close, succeeded, opened }: {
  request: CommandRequest; client: AppServerClient; sessionId: string; current: () => boolean;
  close: () => void; succeeded: () => void; opened: (result: { session: SessionSnapshot; content: UserInputBlock[] }) => void;
}) {
  const [rows, setRows] = useState<Row[]>([]), [query, setQuery] = useState(''), [active, setActive] = useState(0);
  const [busy, setBusy] = useState(true), [error, setError] = useState(''), [detail, setDetail] = useState('');
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
    if (action !== 'fork' && action !== 'branch' && action !== 'retry') throw new Error('Not a historical command.');
    setRows([]);
    if (request.response) {
      const selection = await scope.responseSelection(request.response);
      if (!valid()) return;
      setRows([{ id: request.response.closing_message_id, label: action === 'retry' ? 'Replay the original input once' : 'Continue after this response', choice: { kind: 'history', action, selection } }]);
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
      throw new Error('This message is not an available native user boundary. No mutation was sent.');
    }
  };
  const loadTree = async (offset: number) => {
    setRows([]);
    const tree = await scope.tree(offset); if (!valid()) return;
    setRows(tree.nodes.map(node => ({ id: node.id, label: node.id, detail: `${node.conversation_id} · ${node.origin.type}${node.parent ? ` · parent ${node.parent}` : ''}`, choice: { kind: 'node', nodeId: node.id, conversationId: node.conversation_id } })));
    setNext(tree.next_offset); setActive(0);
  };
  useEffect(() => {
    alive.current = true;
    const load = async () => {
      switch (request.id) {
        case 'model': {
          const result = await scope.models(); if (!valid()) return;
          setDetail(`Current: ${result.current.configured.model}. Choosing a model uses its native defaults.`);
          setRows((result.catalog.models ?? []).flatMap(model => [
            { id: model.model, label: model.model, detail: `Native default${model.defaultReasoningProfile ? ` · ${model.defaultReasoningProfile}` : ''}`, choice: { kind: 'model' as const, model: model.model } },
            ...(model.reasoningProfiles ?? []).map(profile => ({ id: `${model.model}:${profile.id}`, label: `${model.model} · ${profile.id}`, detail: 'Reasoning profile', choice: { kind: 'model' as const, model: model.model, profile: profile.id } })),
          ])); break;
        }
        case 'fork': case 'branch': case 'retry':
          setDetail(request.response && request.id !== 'retry' ? 'The new lineage includes this completed response and opens with an empty composer.' : request.id === 'fork' ? 'Independent Session. Choose the exact User boundary; its prompt returns to the composer.' : request.id === 'retry' ? 'Create a native branch, switch the idle Session to it, and execute the selected prompt once. The original response remains in its original node.' : 'Create a native branch and switch the idle Session to it. The selected prompt returns to the composer.');
          await loadBoundaries(0); break;
        case 'tools': { const result = await scope.tools(); if (valid()) { setDetail(JSON.stringify(result, null, 2)); succeeded(); } break; }
        case 'compact': setDetail('Compact this Session through the native context owner.'); break;
        case 'tree': setDetail('Native Session lineage. Opening another node switches the idle resident runtime; the original history is preserved.'); await loadTree(0); break;
        case 'goal': throw new Error('Goal controls are in the Goal dock.');
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
    } catch (cause) { if (valid()) { setError(`${String(cause)} Close and reread authoritative state before another mutation.`); setStopped(true); } }
    finally { selecting.current = false; if (valid()) setBusy(false); }
  };
  const perform = async () => {
    if (selecting.current || busy || stopped || blocked || !valid()) return;
    selecting.current = true; setBusy(true); setError('');
    try {
      if (request.id === 'compact') { await scope.compact(); if (valid() && scope.current()) { succeeded(); close(); } }
    } catch (cause) { if (valid()) { setError(`${String(cause)} Close and reread authoritative state before another mutation.`); setStopped(true); } }
    finally { selecting.current = false; if (valid()) setBusy(false); }
  };
  const filtered = rows.filter(row => `${row.label} ${row.detail ?? ''}`.toLowerCase().includes(query.toLowerCase()));
  useEffect(() => { options.current?.querySelector('[aria-selected="true"]')?.scrollIntoView?.({ block: 'nearest' }); }, [active, query]);
  const stale = !scope.current();
  return <Modal closeLabel="Close dialog" open title={request.id === 'tree' ? 'Session tree' : request.id === 'retry' ? 'Retry / Regenerate' : `/${request.id}`} onClose={close}>
    <p style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{detail}</p>
    {error && <p role="alert">{error}</p>}
    {blocked && <p role="status">Waiting for native execution and accepted inbound to settle.</p>}
    {!busy && stale && <p role="status">Attachment changed. Close and reopen to read current native state.</p>}
    {busy && <p role="status">Waiting for native acknowledgement…</p>}
    {!busy && !error && !rows.length && (historical || request.id === 'tree' || request.id === 'model') && <p role="status">No native choices available.</p>}
    {request.id === 'compact' && <Button disabled={busy || stopped || stale || blocked} onClick={() => void perform()}>Compact context</Button>}
    {!!rows.length && <><input autoFocus className={css.search} aria-label="Filter options" value={query} disabled={busy}
      onChange={event => { setQuery(event.target.value); setActive(0); }} aria-controls="command-options" aria-activedescendant={filtered[active] ? `choice-${active}` : undefined}
      onKeyDown={event => {
        if ((event.key === 'ArrowDown' || event.key === 'ArrowUp') && filtered.length) { event.preventDefault(); setActive(index => (index + (event.key === 'ArrowDown' ? 1 : filtered.length - 1)) % filtered.length); }
        if (event.key === 'Enter' && filtered[active] && !event.nativeEvent.isComposing) { event.preventDefault(); void choose(filtered[active].choice); }
      }} />
      <div ref={options} className={css.options} id="command-options" role="listbox" aria-label={historical ? 'Historical boundaries' : 'Native choices'}>{filtered.map((row, index) => <button type="button" role="option" id={`choice-${index}`} aria-selected={active === index} disabled={busy || stopped || stale || blocked} key={row.id} className={css.row} onClick={() => void choose(row.choice)}>
        <span>{row.label}</span><small>{row.detail}</small>
      </button>)}</div></>}
    {(historical || request.id === 'tree') && next != null && !request.messageId && <Button disabled={busy} onClick={() => { setBusy(true); void (request.id === 'tree' ? loadTree(next) : loadBoundaries(next)).catch(cause => { if (valid()) setError(String(cause)); }).finally(() => { if (valid()) setBusy(false); }); }}>More {historical ? 'boundaries' : 'nodes'}</Button>}
  </Modal>;
}
