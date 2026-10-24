import type { RuntimeClientJob, ToolExecutionResult, UserMessageBlock } from '../../../protocol/app-server/v57';
import type { ToolDetailsModel } from '../presentation/agent/ToolDetails';
import type { Translate } from '../locale/translation';
import { jobStateLabel } from './status-labels';

const object = (value: unknown): Record<string, unknown> | undefined => value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
export const jobStates: readonly RuntimeClientJob['state'][] = ['starting', 'running', 'cancelling', 'publishing_terminal', 'succeeded', 'failed', 'denied', 'cancelled', 'timed_out', 'outcome_unknown'];
export function jobState(value: unknown): RuntimeClientJob['state'] | undefined {
  return typeof value === 'string' && jobStates.includes(value as RuntimeClientJob['state']) ? value as RuntimeClientJob['state'] : undefined;
}
export interface BackgroundLaunch { jobId: string; tool: string; outputPath: string }
/** The native acceptance receipt proves dispatch, never process exit. */
export function backgroundLaunch(value: unknown, name: string): BackgroundLaunch | undefined {
  const data = object(value);
  return typeof data?.job_id === 'string' && data.state === 'starting' && data.tool === name && typeof data.output_path === 'string'
    ? { jobId: data.job_id, tool: name, outputPath: data.output_path } : undefined;
}
export function launchResult(result: ToolExecutionResult | undefined, name: string): BackgroundLaunch | undefined {
  if (result?.status.type !== 'success') return;
  const content = result.content;
  return content?.length === 1 && content[0]?.type === 'json' ? backgroundLaunch(content[0].value, name) : undefined;
}
export interface JobResultItem { jobId: string; tool: string; state: RuntimeClientJob['state']; output?: string }
export function jobResultOutput(value: unknown): string | undefined {
  const result = object(value), status = object(result?.status);
  const diagnostic = status?.error ?? status?.detail ?? status?.reason;
  const lines = Array.isArray(result?.content) ? result.content.flatMap(block => {
    const item = object(block);
    if (item?.type === 'text' && typeof item.text === 'string') return [item.text];
    if (item?.type === 'json') { const data = object(item.value); return [typeof data?.combined === 'string' ? data.combined : JSON.stringify(item.value, null, 2)]; }
    return [];
  }) : [];
  return [typeof diagnostic === 'string' ? diagnostic : undefined, ...lines].filter(Boolean).join('\n') || undefined;
}
export function jobResultItems(value: unknown): JobResultItem[] | undefined {
  const data = object(value);
  const entries = Array.isArray(data?.jobs) ? data.jobs : [data];
  const rows: JobResultItem[] = [];
  for (const entry of entries) {
    const row = object(entry), state = jobState(row?.state);
    if (typeof row?.job_id !== 'string' || typeof row.tool !== 'string' || !state) return;
    rows.push({ jobId: row.job_id, tool: row.tool, state, output: jobResultOutput(row.result) });
  }
  return rows;
}
/** Harness job output/list layout, populated only by the recorded native snapshot. */
export function jobDetails(items: readonly JobResultItem[], tx: Translate): ToolDetailsModel {
  return { items: items.map(item => ({ title: item.jobId,
    badge: { label: jobStateLabel(tx, item.state), tone: item.state === 'succeeded' ? 'success' : ['failed', 'denied', 'timed_out'].includes(item.state) ? 'error' : ['cancelled', 'outcome_unknown', 'cancelling'].includes(item.state) ? 'warning' : 'info' },
    code: item.output,
  })), empty: tx('common:activity.no-jobs') };
}
/** Only the runtime's exact terminal-message identity authorizes this format. */
export function backgroundNotice(message: UserMessageBlock): (JobResultItem & { output: string }) | undefined {
  if (message.source !== 'runtime' || message.kind && message.kind !== 'message' || message.content[0]?.type !== 'text' || message.content.slice(1).some(block => block.type !== 'file')) return;
  const text = message.content[0].text;
  const match = /^Background execution (\S+) \(([^\n]+)\) settled: (\w+)(?:\n\nResult:\n([\s\S]*))?$/.exec(text);
  if (!match || message.id !== `background-${match[1]}-terminal`) return;
  const state = jobState(match[3]);
  if (!state || ['starting', 'running', 'cancelling', 'publishing_terminal'].includes(state)) return;
  return { jobId: match[1]!, tool: match[2]!, state, output: match[4] ?? '' };
}
