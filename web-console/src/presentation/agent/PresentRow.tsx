import { ToolInspectionButton } from './ToolCard';
import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-deliverables PresentRow: the call row whose
// collapsed summary is the delivery status plus the declared paths, and whose
// expansion is the recorded result text. Harness's preparing/started split,
// Cordis tool-view props are excluded; native Trace navigation owns Inspect; the rustX
// adapter (bindings/present.ts) supplies the native lifecycle phase.
import { useState } from 'react';
import { DisclosureRow } from '../primitives/DisclosureRow';
import { IconDeliverDocOutline14 } from '../primitives/icons';
import css from './Tool.module.css';
import tree from './ToolTree.module.css';
import present from './PresentRow.module.css';

/** One present call's native phase, declared paths and recorded result text. */
export interface PresentRowView {
  readonly id: string;
  readonly phase: 'preparing' | 'running' | 'ok' | 'error' | 'stopped';
  /** Paths from the published arguments: intent, never a delivery. */
  readonly paths: string;
  /** Result or failure text; empty before settlement. */
  readonly details: string;
}

export function PresentRow({ row, inspect }: { row: PresentRowView; inspect?: () => Promise<void> }) {
  const tx = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const expandable = row.details !== '';
  return <div className={tree.callRow} data-tool-call-id={row.id} data-tool-name="present">
    <div className={css.root} data-tool="present" data-state={row.phase === 'preparing' ? 'running' : row.phase} data-present-phase={row.phase}
      aria-label={row.phase === 'preparing' ? tx('tools:present.preparing') : undefined}>
      <DisclosureRow rowClassName={css.row} leadingClassName={css.leading} titleClassName={css.title} chevronClassName={css.chevron}
        icon={<IconDeliverDocOutline14/>} title={tx('tools:present.title')} open={expanded && expandable} expandable={expandable}
        expandOnRowClick keepContentWhenOpen onToggle={() => setExpanded(value => !value)}
        collapsedContent={<><span className={css.sep} aria-hidden/><span className={css.summary}>
          <span className={present.status}>{tx(`tools:present.${row.phase}`)}</span>
          {row.phase !== 'preparing' && <span className={present.paths}>{row.paths}</span>}
        </span></>}>
        {expanded && expandable && <div className={css.bodyWrap}><pre className={present.output}>{row.details}</pre>{inspect && <ToolInspectionButton inspect={inspect}/>}</div>}
      </DisclosureRow>
    </div>
  </div>;
}
