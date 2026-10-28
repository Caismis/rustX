import type { SourceTarget, WorkflowAnalysisQuery, WorkflowAnalysisReport } from './v60.ts';

/** Wire-default normalization only; scope and type proofs remain native. */
function queryIdentity(query: WorkflowAnalysisQuery | null | undefined): unknown {
  return query == null ? null : { ...query, search: query.search ?? '', reference: query.reference ?? null,
    consumer: { ...query.consumer, path: query.consumer.path ?? [] } };
}
function same(left: unknown, right: unknown): boolean {
  if (left === right) return true;
  if (!left || !right || typeof left !== 'object' || typeof right !== 'object' || Array.isArray(left) !== Array.isArray(right)) return false;
  if (Array.isArray(left)) return left.length === (right as unknown[]).length && left.every((value, index) => same(value, (right as unknown[])[index]));
  const a = Object.entries(left).filter(([, value]) => value !== undefined), b = right as Record<string, unknown>;
  return a.length === Object.values(b).filter(value => value !== undefined).length && a.every(([key, value]) => Object.hasOwn(b, key) && same(value, b[key]));
}
export function sameAnalysisQuery(left: WorkflowAnalysisQuery | null | undefined, right: WorkflowAnalysisQuery | null | undefined): boolean {
  return same(queryIdentity(left), queryIdentity(right));
}
export function analysisMatches(report: WorkflowAnalysisReport, target: SourceTarget, id: string, generation: string, query?: WorkflowAnalysisQuery): boolean {
  return same(report.target, target) && report.id === id && report.generation === generation && sameAnalysisQuery(report.query, query)
    && /^sha256:[a-f0-9]{64}$/.test(report.candidate_revision);
}
