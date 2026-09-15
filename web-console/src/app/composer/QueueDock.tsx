/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-conversation QueueDock: one row renders
// directly, several rows default to a collapsible count header, and an accepted
// submission stays visibly provisional. The native inbound mailbox owns every
// row; a provisional row exists only once acceptance named its MessageId. No
// edit, remove or per-row steer control exists here (WEB-06).
import { useEffect, useId, useState } from 'react';
import { contentPreview, inboundOrigin, type InboundRow } from '../../bindings/composer-context';
import type { Submission } from '../../client/app-server';
import { IconChevronDownOutline14, IconQueueOutline14 } from '../../presentation/primitives/icons';
import css from './QueueDock.module.css';

const QueueGlyph = () => <span className={css.lead} aria-hidden><IconQueueOutline14 /></span>;
const ACCEPTED = 'Accepted · awaiting projection';

export function QueueDock({ rows, submissions, running }: {
  rows: readonly InboundRow[];
  /** Accepted submissions keyed by their server MessageId; the client settles each by that id. */
  submissions: readonly Submission[];
  running: boolean;
}) {
  const [collapsed, setCollapsed] = useState(true);
  const listId = useId();
  // An ordinary idle send is admitted directly, not queued: its echo is not a queue row.
  const echoes = running ? submissions : [];
  const count = rows.length + echoes.length;
  useEffect(() => { if (count === 0) setCollapsed(true); }, [count]);
  if (count === 0) return null;
  const listed = count === 1 || !collapsed;
  const status = running ? 'next safe boundary of the running attempt' : 'awaiting admission';
  const hiddenEchoes = !listed && echoes.length > 0;
  return <section className={css.dock} aria-label="Queue" data-queue-dock="">
    <div className={css.panel}>
      {count > 1 && <button type="button" className={css.header} aria-expanded={!collapsed} aria-controls={listed ? listId : undefined} onClick={() => setCollapsed(value => !value)}>
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
            <span className={css.status}>{count === 1 ? `#${row.sequence} · ${status}` : `#${row.sequence}`}</span>
          </li>;
        })}
        {echoes.map(submission => <li key={`accepted:${submission.messageId}`} className={`${css.row} ${css.pendingRow}`} data-submission-echo="" data-accepted-message-id={submission.messageId}>
          {count === 1 && <QueueGlyph />}
          <span className={css.preview}>{contentPreview(submission.content)}</span>
          <span className={css.status} role="status">{ACCEPTED}</span>
        </li>)}
      </ul>}
    </div>
  </section>;
}
