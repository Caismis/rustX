import { useSyncExternalStore } from 'react';
import type { ConfigRepairReceipt, SourceTarget } from '../../../protocol/app-server/v57';
import type { AppServerClient } from '../client/app-server';
import { useClientSelector } from '../client/selectors';
import { useTranslation } from '../locale/react';
import type { Translate } from '../locale/translation';
import { Button } from '../presentation/primitives/Button';
import { repairCount } from './settings/advanced/AutoRepair';
import { mostSevere, type RepairAcknowledgements, repairIdentity, unacknowledgedRepairs } from './repair-notices';
import type { RepairInspection } from './settings/machines/navigation';

/** The one concise sentence for newly observed receipts of distinct sources:
 * a failure is named by its most severe receipt, otherwise every committed
 * repair is counted. */
export function repairNoticeText(tx: Translate, receipts: readonly ConfigRepairReceipt[]): string {
  const owner = (target: SourceTarget) => target.kind === 'user' ? 'user' : 'workspace';
  const failed = mostSevere(receipts, receipt => receipt);
  if (failed && failed.status !== 'repaired') {
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
 * unchanged source. A combined notice is about its most severe receipt —
 * `commit_uncertain`, then `unresolved`, then `repaired` — so an uncertain
 * publication is never hidden behind a repair that published nothing. View
 * details asks Settings to open the exact source owner
 * at Advanced → Auto Repair; that receipt alone is acknowledged once Settings
 * has presented its details, so a combined notice keeps presenting the other
 * source, and a failed or abandoned navigation acknowledges nothing. Dismiss
 * acknowledges every receipt presented, immediately. */
export function RepairNotice({ client, viewDetails, acknowledgements }: {
  client: AppServerClient;
  viewDetails: (inspection: RepairInspection) => void;
  acknowledgements: RepairAcknowledgements;
}) {
  const tx = useTranslation();
  const observations = useClientSelector(client, state => state.sourceRepairs);
  // Re-render whenever the acknowledgements change, here or in another tab.
  useSyncExternalStore(acknowledgements.subscribe, acknowledgements.version);
  const repairs = unacknowledgedRepairs(observations, acknowledgements);
  if (repairs.length === 0) return null;
  const receipts = repairs.map(repair => repair.receipt);
  // The most severe receipt is the notice's subject: its wording, its role
  // and the source View details opens.
  const subject = mostSevere(repairs, repair => repair.receipt)!;
  const failed = subject.receipt.status !== 'repaired';
  return <div className={`repair-notice${failed ? ' error' : ''}`} role={failed ? 'alert' : 'status'} aria-live={failed ? 'assertive' : 'polite'} aria-label={tx('settings:auto-repair.toast-label')}>
    <span>{repairNoticeText(tx, receipts)}</span>
    <Button size="sm" onClick={() => viewDetails({ ...repairIdentity(subject), source: subject.receipt.target })}>{tx('settings:auto-repair.view-details')}</Button>
    <Button size="sm" variant="ghost" onClick={() => acknowledgements.acknowledge(repairs.map(repairIdentity))}>{tx('settings:auto-repair.dismiss')}</Button>
  </div>;
}
