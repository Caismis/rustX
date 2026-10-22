import { useTranslation } from '../../../locale/react';
import type { WorkflowFailure } from '../port';

const short = (revision: string) => revision.replace(/^sha256:/, '').slice(0, 8);

/** The localized text of one typed authoring failure. Native diagnostic
 * reasons and paths are shown as native facts. */
export function FailureText({ failure, operation }: { failure: WorkflowFailure; operation?: string }) {
  const tx = useTranslation();
  switch (failure.kind) {
    case 'conflict': return <>{tx('workflows:notice.conflict', { p0: short(failure.expected), p1: short(failure.actual) })}</>;
    case 'invalid': return <>{tx('workflows:notice.invalid', { p0: failure.diagnostic.path, p1: failure.diagnostic.reason })}</>;
    case 'uncertain': return <>{tx('workflows:notice.uncertain')}</>;
    case 'unavailable': return <>{tx('workflows:notice.unavailable')}</>;
    case 'failed': return <>{tx('workflows:notice.failed', { p0: operation ?? '', p1: failure.message })}</>;
  }
}

export { short as shortRevision };
