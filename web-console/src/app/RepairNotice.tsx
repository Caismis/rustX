import { useMemo, useState } from 'react';
import type { ConfigRepairReceipt, SourceTarget } from '../../../protocol/app-server/v46';
import type { AppServerClient } from '../client/app-server';
import { useClientSelector } from '../client/selectors';
import { useTranslation } from '../locale/react';
import type { Translate } from '../locale/translation';
import { Button } from '../presentation/primitives/Button';
import { repairCount } from './settings/advanced/AutoRepair';
import { RepairAcknowledgements, unacknowledgedRepairs } from './repair-notices';

/** The one concise sentence for newly observed receipts of distinct sources:
 * a failure is named first, otherwise every committed repair is counted. */
export function repairNoticeText(tx: Translate, receipts: readonly ConfigRepairReceipt[]): string {
  const owner = (target: SourceTarget) => target.kind === 'user' ? 'user' : 'workspace';
  const failed = receipts.find(receipt => receipt.status !== 'repaired');
  if (failed) {
    return tx(failed.status === 'unresolved'
      ? `settings:auto-repair.toast-unresolved-${owner(failed.target)}`
      : `settings:auto-repair.toast-uncertain-${owner(failed.target)}`);
  }
  const parts = receipts.map(receipt => tx(`settings:auto-repair.toast-source-${owner(receipt.target)}`, { count: repairCount(tx, receipt.changed_count) }));
  const summary = parts.length > 1 ? tx('settings:auto-repair.toast-both', { first: parts[0], second: parts.slice(1).join(', ') }) : parts[0];
  return tx('settings:auto-repair.toast-repaired', { summary });
}

/** A brief, nonblocking notification of newly committed — or failed —
 * automatic configuration repairs (Issue #464).
 *
 * It presents native receipts the client observed and nothing else: no
 * confirmation, no forced navigation, and nothing at all for a healthy
 * unchanged source. View details opens the exact source owner at Settings →
 * Advanced → Auto Repair; both actions acknowledge the receipts presented. */
export function RepairNotice({ client, viewDetails, acknowledgements }: {
  client: AppServerClient;
  viewDetails: (source: SourceTarget) => void;
  acknowledgements?: RepairAcknowledgements;
}) {
  const tx = useTranslation();
  const ledger = useMemo(() => acknowledgements ?? new RepairAcknowledgements(), [acknowledgements]);
  const observations = useClientSelector(client, state => state.sourceRepairs);
  // Re-render after an acknowledgement; the ledger itself is the fact.
  const [, setRevision] = useState(0);
  const receipts = unacknowledgedRepairs(observations, ledger);
  if (receipts.length === 0) return null;
  const acknowledge = () => { ledger.acknowledge(receipts.map(receipt => receipt.id)); setRevision(value => value + 1); };
  const failed = receipts.some(receipt => receipt.status !== 'repaired');
  const subject = receipts.find(receipt => receipt.status !== 'repaired') ?? receipts[0];
  return <div className={`repair-notice${failed ? ' error' : ''}`} role={failed ? 'alert' : 'status'} aria-live={failed ? 'assertive' : 'polite'} aria-label={tx('settings:auto-repair.toast-label')}>
    <span>{repairNoticeText(tx, receipts)}</span>
    <Button size="sm" onClick={() => { acknowledge(); viewDetails(subject.target); }}>{tx('settings:auto-repair.view-details')}</Button>
    <Button size="sm" variant="ghost" onClick={acknowledge}>{tx('settings:auto-repair.dismiss')}</Button>
  </div>;
}
