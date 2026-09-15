/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-conversation QueueDock: one row renders
// directly, several rows default to a collapsible count header, and a local
// submission echo stays visibly provisional. The native inbound mailbox owns
// every row. No edit, remove or per-row steer control exists here (WEB-06).
import { useEffect, useId, useState } from 'react';
import { contentPreview, inboundOrigin, type InboundRow } from '../../bindings/composer-context';
import type { Submission } from '../../client/app-server';
import { IconChevronDownOutline14, IconQueueOutline14 } from '../../presentation/primitives/icons';
import css from './QueueDock.module.css';

const QueueGlyph = () => <span className={css.lead} aria-hidden><IconQueueOutline14 /></span>;
/** Unacknowledged echoes are still sending; acknowledged ones wait for their exact MessageId. */
const echoState = (echoes: readonly Submission[]) => echoes.some(echo => !echo.messageId) ? 'Sending…' : 'Accepted · awaiting projection';

export function QueueDock({ rows, submissions, running }: {
  rows: readonly InboundRow[];
  /** Provisional submissions; the client settles each only by its exact accepted MessageId. */
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
  return <section className={css.dock} aria-label="Queue" data-queue-dock="">
    <div className={css.panel}>
      {count > 1 && <button type="button" className={css.header} aria-expanded={!collapsed} aria-controls={listed ? listId : undefined} onClick={() => setCollapsed(value => !value)}>
        <QueueGlyph /><span className={css.count}>{count} queued</span>
        {/* A collapsed header keeps provisional submissions visible. */}
        <span className={css.status} role={!listed && echoes.length ? 'status' : undefined}>{!listed && echoes.length ? echoState(echoes) : status}</span>
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
        {echoes.map(submission => <li key={`submission:${submission.key}`} className={`${css.row} ${css.pendingRow}`} data-submission-echo="">
          {count === 1 && <QueueGlyph />}
          <span className={css.preview}>{contentPreview(submission.content)}</span>
          <span className={css.status} role="status">{echoState([submission])}</span>
        </li>)}
      </ul>}
    </div>
  </section>;
}
