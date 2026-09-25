/* Copyright (c) 2026 DeepSeek. MIT. Adapted ContextInjectionRow chrome; native status semantics. See PROVENANCE.md. */
import { useState } from 'react';
import type { AgentStatusView } from '../../../../protocol/app-server/v24';
import { agentStatusFacets, agentStatusSummary } from '../../bindings/agent-status';
import { DisclosureRow } from '../../presentation/primitives/DisclosureRow';
import { IconContextInjectionOutline16 } from '../../presentation/primitives/icons';
import css from './AgentStatus.module.css';

/** One composed Agent Status, drawn subordinate to the runtime-published anchor
 * the transcript placed it at.
 *
 * It is a contextual annotation, not a conversation speaker: no bubble, no user
 * band, no response actions, and no current-state panel. Everything it shows comes
 * from the runtime's closed typed section vocabulary; `rendered` is diagnostics and
 * is never parsed here. The caller owns placement — this component never looks at
 * neighbouring rows, timestamps or array positions. */
export function AgentStatusAnnotation({ status }: { status: AgentStatusView }) {
  const [expanded, setExpanded] = useState(false);
  const facets = agentStatusFacets(status);
  const summary = agentStatusSummary(status);
  return <div className={css.root} role="note" aria-label="Agent Status" data-agent-status={status.status_message_id}>
    <DisclosureRow rowClassName={css.row} leadingClassName={css.leading} titleClassName={css.title} chevronClassName={css.chevron}
      icon={<IconContextInjectionOutline16 size={14} />} title="Agent Status" open={expanded} expandable={facets.length > 0} expandOnRowClick
      keepContentWhenOpen onToggle={() => setExpanded(value => !value)}
      collapsedContent={summary ? <><span className={css.sep} aria-hidden/><span className={css.summary}>{summary}</span></> : undefined}>
      <dl className={css.sections}>{facets.map(facet => <div key={facet.kind} className={css.section} data-status-section={facet.kind}>
        <dt className={css.label}>{facet.label}</dt>
        <dd className={css.values}>{facet.values.map((value, index) => <span key={index}>{value}</span>)}</dd>
      </div>)}</dl>
    </DisclosureRow>
  </div>;
}
