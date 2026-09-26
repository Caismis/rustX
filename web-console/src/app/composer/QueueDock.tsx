/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-conversation QueueDock: one row renders
// directly, several rows default to a collapsible count header, and an accepted
// submission stays visibly provisional. Native durable Pending Inbound owns every
// row; a provisional row exists only once acceptance named its MessageId. No
// per-row delivery class exists in the native queue.
import { useEffect, useId, useRef, useState } from 'react';
import { contentPreview, inboundOrigin, type InboundRow } from '../../bindings/composer-context';
import type { InboundControlOutcome, Submission } from '../../client/app-server';
import { IconChevronDownOutline14, IconQueueOutline14 } from '../../presentation/primitives/icons';
import type { PendingInboundRef, RuntimeClientSnapshot } from '../../../../protocol/app-server/v23';
import css from './QueueDock.module.css';

const QueueGlyph = () => <span className={css.lead} aria-hidden><IconQueueOutline14 /></span>;
const ACCEPTED = 'Queued · updating…';

export function QueueDock({ rows, submissions, running, disabled = false, edit, remove, observation }: {
  rows: readonly InboundRow[];
  /** Accepted submissions keyed by their server MessageId; the client settles each by that id. */
  submissions: readonly Submission[];
  running: boolean;
  disabled?: boolean;
  observation?: RuntimeClientSnapshot;
  edit?: (expected: PendingInboundRef, text: string) => Promise<InboundControlOutcome>;
  remove?: (expected: PendingInboundRef) => Promise<InboundControlOutcome>;
}) {
  const [collapsed, setCollapsed] = useState(true);
  const listId = useId();
  const alive = useRef(true);
  const latestObservation = useRef(observation);
  latestObservation.current = observation;
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const [draft, setDraft] = useState<{ expected: PendingInboundRef; text: string }>();
  const [operation, setOperation] = useState<{ status: 'pending' | 'uncertain' | 'readback'; observation?: RuntimeClientSnapshot; clearDraft?: boolean }>();
  const [notice, setNotice] = useState('');
  useEffect(() => {
    if (operation && operation.status !== 'pending' && observation !== operation.observation) {
      if (operation.clearDraft) setDraft(undefined);
      setOperation(undefined); setNotice('');
    }
  }, [observation, operation]);
  const currentDraft = draft && rows.find(row => row.sequence === draft.expected.sequence && row.message.id === draft.expected.message_id);
  const staleDraft = !!draft && (!currentDraft || currentDraft.revision !== draft.expected.revision);
  const locked = disabled || !!operation;
  const expected = (row: InboundRow): PendingInboundRef => ({ sequence: row.sequence, message_id: row.message.id, revision: row.revision });
  const apply = async (work: () => Promise<InboundControlOutcome>, editing = false) => {
    if (locked) return;
    setOperation({ status: 'pending', observation }); setNotice('');
    const result = await work();
    if (!alive.current || result.status === 'obsolete') return;
    if (result.status === 'uncertain') {
      setOperation({ status: 'uncertain', observation: latestObservation.current });
      setNotice('Outcome uncertain. Do not resend; reconnect to read the authoritative queue.');
      return;
    }
    setOperation(result.observed ? undefined : { status: 'readback', observation: latestObservation.current, clearDraft: editing && result.status === 'known' && result.outcome.status === 'applied' });
    if (result.status === 'rejected') { setNotice(result.reason); return; }
    if (result.outcome.status === 'applied') {
      if (editing && result.observed) setDraft(undefined);
      setNotice(result.observed ? '' : 'Committed; awaiting authoritative readback.');
    } else {
      setNotice(result.outcome.status === 'conflict' ? 'This item changed. Your draft is preserved; compare it with the current row.'
        : result.outcome.status === 'not_pending' ? 'This occurrence is no longer pending. Your draft is preserved.'
        : 'This item cannot be edited with the text editor.');
    }
  };
  // An ordinary idle send is admitted directly, not queued: its echo is not a queue row.
  const echoes = running ? submissions : [];
  const count = rows.length + echoes.length;
  useEffect(() => { if (count === 0) setCollapsed(true); }, [count]);
  if (count === 0 && !draft && !notice) return null;
  const listed = count === 1 || !collapsed || !!draft || !!operation;
  const status = running ? 'Queued' : 'Waiting to start';
  const hiddenEchoes = !listed && echoes.length > 0;
  return <section className={css.dock} aria-label="Queue" data-queue-dock="">
    <div className={css.panel}>
      {count > 1 && <button type="button" className={css.header} aria-expanded={listed} disabled={!!draft || !!operation} aria-controls={listed ? listId : undefined} onClick={() => setCollapsed(value => !value)}>
        <QueueGlyph /><span className={css.count}>{count} queued</span>
        {/* A collapsed header keeps accepted provisional submissions visible. */}
        <span className={css.status} role={hiddenEchoes ? 'status' : undefined}>{hiddenEchoes ? `${echoes.length} ${ACCEPTED.toLowerCase()}` : status}</span>
        <span className={css.chevron} data-collapsed={collapsed} aria-hidden><IconChevronDownOutline14 /></span>
      </button>}
      {listed && <ul id={listId} className={css.list}>
        {rows.map(row => {
          const origin = inboundOrigin(row.message);
          return <li key={`inbound:${row.sequence}`} className={css.row} data-inbound-sequence={row.sequence} data-message-id={row.message.id}>
            {count === 1 && <QueueGlyph />}
            <span className={css.preview} title={contentPreview(row.message.content)}>{contentPreview(row.message.content)}</span>
            {origin && <span className={css.status}>{origin}</span>}
            {row.message.source === 'human' && (!row.message.kind || row.message.kind === 'message') && <div className={css.actions}>
              <button type="button" className={css.action} disabled={locked || !edit || row.message.content.length !== 1 || row.message.content[0].type !== 'text'}
                title={row.message.content.length !== 1 || row.message.content[0].type !== 'text' ? 'Non-text or multiple content blocks cannot be safely edited here.' : 'Edit queued message'}
                onClick={() => { setDraft({ expected: expected(row), text: row.message.content[0].type === 'text' ? row.message.content[0].text : '' }); setNotice(''); }}>Edit</button>
              <button type="button" className={css.action} disabled={locked || !remove} onClick={() => { if (remove) void apply(() => remove(expected(row))); }}>Remove</button>
            </div>}
            {count === 1 && <span className={css.status}>{status}</span>}
          </li>;
        })}
        {echoes.map(submission => <li key={`accepted:${submission.messageId}`} className={`${css.row} ${css.pendingRow}`} data-submission-echo="" data-accepted-message-id={submission.messageId}>
          {count === 1 && <QueueGlyph />}
          <span className={css.preview}>{contentPreview(submission.content)}</span>
          <span className={css.status} role="status">{ACCEPTED}</span>
        </li>)}
      </ul>}
      {draft && <div className={css.editPanel}>
        <label>Edit queued message<input className={css.editor} aria-label="Edit queued message" value={draft.text} disabled={locked}
          onChange={event => setDraft({ ...draft, text: event.currentTarget.value })}
          onKeyDown={event => {
            if (event.key === 'Escape' && !locked) setDraft(undefined);
            if (event.key === 'Enter' && !event.nativeEvent.isComposing && !locked && !staleDraft && draft.text.trim() && edit) { event.preventDefault(); void apply(() => edit(draft.expected, draft.text), true); }
          }} /></label>
        <div className={css.actions}>
          <button type="button" className={css.action} disabled={locked || staleDraft || !draft.text.trim() || !edit} onClick={() => { if (edit) void apply(() => edit(draft.expected, draft.text), true); }}>Save</button>
          <button type="button" className={css.action} disabled={locked} onClick={() => { setDraft(undefined); setNotice(''); }}>Cancel edit</button>
          {staleDraft && currentDraft && <button type="button" className={css.action} disabled={locked} onClick={() => setDraft({ ...draft, expected: expected(currentDraft) })}>Use latest version</button>}
        </div>
        {staleDraft && <p role="status">{currentDraft ? 'This queued message changed. Review the latest version before saving.' : 'This message is no longer queued.'}</p>}
      </div>}
      {operation?.status === 'pending' && <p className={css.notice} role="status">Updating queue…</p>}
      {notice && <p className={css.notice} role="status">{notice}</p>}
    </div>
  </section>;
}
