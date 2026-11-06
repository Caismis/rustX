import type { JobResultItem } from '../../bindings/jobs';
import { jobDetails } from '../../bindings/jobs';
import { useTranslation } from '../../locale/react';
import { ToolDetails } from '../../presentation/agent/ToolDetails';

/** Immutable recorded results, never patched from the current Jobs roster. */
export function JobResultDetails({ items }: { items: readonly JobResultItem[] }) {
  const tx = useTranslation();
  return <ToolDetails model={jobDetails(items, tx)}/>;
}
