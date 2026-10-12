import { useEffect, useRef } from 'react';
import { useTranslation } from '../../../locale/react';
import type { Translate } from '../../../locale/translation';
import type { ConfigRepairChange, ConfigRepairReceipt } from '../../../../../protocol/app-server/v46';
import css from '../../../presentation/settings/SettingsContent.module.css';

/** One redacted native repair change, worded. Never a value: native receipts
 * carry field paths, Tool names and version numbers only. */
export function repairChangeLabel(tx: Translate, change: ConfigRepairChange): string {
  switch (change.kind) {
    case 'removed_field': return tx('settings:auto-repair.removed-field', { path: change.field_path });
    case 'removed_native_tool': return tx(`settings:auto-repair.removed-tool-${change.reason}`, { name: change.name });
    case 'updated_schema_version': return tx('settings:auto-repair.updated-schema-version', { from: change.from, to: change.to });
  }
}

/** A worded change count. */
export function repairCount(tx: Translate, count: number): string {
  return count === 1 ? tx('settings:auto-repair.count-one') : tx('settings:auto-repair.count-other', { count });
}

/** Advanced → Auto Repair: this owner's latest native automatic repair of its
 * own `rustx.toml` (Issue #464).
 *
 * It renders the receipt carried by the target's authoritative source read and
 * nothing else. It owns no repair rule, writes nothing and touches no editing
 * transaction: a receipt arriving while a unit holds an unsaved draft changes
 * what this section says, never the draft, its CAS base or its submission. */
export function AutoRepairSection({ receipt, malformed, focused }: {
  receipt?: ConfigRepairReceipt | null;
  /** Whether the document still fails its source contract, so the manual
   * repair editor is the remaining way to correct it. */
  malformed: boolean;
  /** Opened from a repair notification's View details. */
  focused: boolean;
}) {
  const tx = useTranslation();
  const section = useRef<HTMLElement>(null);
  useEffect(() => { if (focused) section.current?.scrollIntoView?.({ block: 'start' }); }, [focused]);
  return <section ref={section} id="settings-auto-repair" aria-label={tx('settings:auto-repair.title')} data-focused={focused || undefined}>
    <h4>{tx('settings:auto-repair.title')}</h4>
    <p className={css.hint}>{tx('settings:auto-repair.intro')}</p>
    {!receipt ? <p role="status">{tx('settings:auto-repair.none')}</p> : <>
      {receipt.status === 'repaired'
        ? <p role="status">{tx('settings:auto-repair.repaired', { count: repairCount(tx, receipt.changed_count) })}</p>
        : <p role="alert" className={css.error}>{receipt.status === 'unresolved' ? tx('settings:auto-repair.unresolved') : tx('settings:auto-repair.commit-uncertain')}</p>}
      {receipt.diagnostic && <p className={css.error}>{receipt.diagnostic}</p>}
      {receipt.changes.length > 0 && <>
        <h5>{receipt.status === 'unresolved' ? tx('settings:auto-repair.not-applied') : tx('settings:auto-repair.changes')}</h5>
        <ul aria-label={receipt.status === 'unresolved' ? tx('settings:auto-repair.not-applied') : tx('settings:auto-repair.changes')}>
          {receipt.changes.map((change, index) => <li key={index}>{repairChangeLabel(tx, change)}</li>)}
        </ul>
        {receipt.truncated && <p className={css.hint}>{tx('settings:auto-repair.truncated', { shown: receipt.changes.length, total: receipt.changed_count })}</p>}
      </>}
      <p>{receipt.backup_available ? tx('settings:auto-repair.backup-kept') : tx('settings:auto-repair.backup-none')}</p>
      <p className={css.hint}>{receipt.after_revision
        ? tx('settings:auto-repair.revisions', { before: receipt.before_revision, after: receipt.after_revision })
        : tx('settings:auto-repair.revision', { before: receipt.before_revision })}</p>
      {receipt.status !== 'repaired' && malformed && <p>{tx('settings:auto-repair.manual-repair')}</p>}
    </>}
    <p className={css.hint}>{tx('settings:auto-repair.not-application')}</p>
  </section>;
}
