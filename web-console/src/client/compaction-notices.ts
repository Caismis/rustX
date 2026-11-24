import type { RuntimeClientCompactionOperation, RuntimeClientContextView } from '../../../protocol/app-server/v82';

export interface CompactionNoticeEvidence {
  conversation: string;
  activation?: string;
  observed: ReadonlySet<string>;
  dismissed: ReadonlySet<string>;
}

export function inactiveCompaction({ status, live }: RuntimeClientCompactionOperation) {
  return !live && ['generating', 'retrying', 'foreground_waiting', 'ready'].includes(status.phase);
}

export function compactionNoticeKey({ status }: RuntimeClientCompactionOperation) {
  return `${status.operation_id}:${status.admission_generation}`;
}

/** Keep presentation evidence for the native retained operations. A fresh
 * snapshot containing both a checkpoint and a failure may have either order. */
export function observeCompactionNotices(previous: CompactionNoticeEvidence | undefined, conversation: string, context: RuntimeClientContextView | undefined): CompactionNoticeEvidence | undefined {
  const prior = previous?.conversation === conversation ? previous : undefined;
  if (!context) return prior;
  const operations = context.compaction_operations ?? [];
  const activation = context.projection_head?.activation_id ?? context.latest_compaction?.activation_id;
  const advanced = activation != null && activation !== prior?.activation && BigInt(activation) > BigInt(prior?.activation ?? '0');
  const committed = operations.filter(({ status }) => status.phase === 'committed');
  const observed = new Set<string>(), dismissed = new Set<string>();
  for (const operation of operations) {
    const { status } = operation;
    if (!inactiveCompaction(operation) && !['failed', 'cancelled', 'superseded', 'no_effect'].includes(status.phase)) continue;
    const key = compactionNoticeKey(operation);
    observed.add(key);
    if (prior?.dismissed.has(key) || (advanced && prior?.observed.has(key))
      || committed.some(item => BigInt(item.status.admission_generation) > BigInt(status.admission_generation))) dismissed.add(key);
  }
  return { conversation, activation: activation ?? prior?.activation, observed, dismissed };
}
