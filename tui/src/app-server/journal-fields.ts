/** Native indexed fields referenced by the current immutable attempt display. */
import type { ForegroundToolExecution, JournalFieldRef, RuntimeClientAttemptPhase } from '../../../protocol/app-server/v57.ts';

export interface DeferredJournalField {
  label: 'Tool result' | 'Attempt failure' | 'Finish reason' | 'Turn attempt identity';
  reference: JournalFieldRef;
}

export function deferredJournalFields(attempt: {
  phase: RuntimeClientAttemptPhase;
  foreground?: readonly ForegroundToolExecution[];
} | null | undefined): DeferredJournalField[] {
  if (!attempt) return [];
  const fields: DeferredJournalField[] = (attempt.foreground ?? []).flatMap(tool =>
    tool.result_source ? [{ label: 'Tool result' as const, reference: tool.result_source }] : []);
  if (attempt.phase.type === 'settled') {
    const outcome = attempt.phase.outcome;
    if (outcome.type === 'failed' && outcome.error_source) {
      fields.push({ label: 'Attempt failure', reference: outcome.error_source });
    } else if (outcome.type === 'completed' && outcome.finish_reason_source) {
      fields.push({ label: 'Finish reason', reference: outcome.finish_reason_source });
    }
  }
  return fields;
}
