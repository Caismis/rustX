import type { Translate } from '../locale/translation';
import type { ToolDetailsModel } from '../presentation/agent/ToolDetails';
import { jobDetails, jobResultItems } from './jobs';
import { jobStateLabel } from './status-labels';

const object = (value: unknown): Record<string, unknown> | undefined => value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
const count = (value: unknown): value is number => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;

/** Finite native result formats. Unsupported/incomplete values retain the ordinary IN/OUT view. */
export function recordedToolDetails(toolId: string | null | undefined, content: readonly { type: string; value?: unknown; text?: string }[] | undefined, tx: Translate): ToolDetailsModel | undefined {
  const blocks = content ?? [], block = blocks.length === 1 ? blocks[0] : undefined;
  const value = block?.type === 'json' ? block.value : undefined, data = object(value);
  if (['tool-job_list', 'tool-job_status', 'tool-job_wait', 'tool-job_cancel'].includes(toolId ?? '')) {
    const items = jobResultItems(value);
    if (!items) return;
    const details = jobDetails(items, tx), first = items[0];
    return { ...details, items: toolId === 'tool-job_list' ? details.items.map((item, index) => ({ ...item, fields: [{ label: tx('tools:detail.type'), value: items[index]!.tool }] })) : details.items,
      summary: toolId === 'tool-job_list' ? tx('common:activity.job-count', { count: items.length }) : first ? `${first.jobId} · ${jobStateLabel(tx, first.state)}` : undefined,
      expandedSummary: toolId === 'tool-job_list' ? undefined : first?.jobId,
      caption: data?.truncated === true && count(data.returned) && count(data.matched) ? tx('tools:detail.bounded', { returned: data.returned, matched: data.matched }) : undefined };
  }
  if (['native.create_goal', 'native.get_goal', 'native.update_goal'].includes(toolId ?? '')) {
    if (block?.type !== 'json') return;
    if (value === null) return { items: [], empty: tx('tools:detail.goal-empty') };
    if (!data || typeof data.objective !== 'string' || !['active', 'paused', 'blocked', 'complete'].includes(String(data.phase)) || !count(data.autonomous_rounds_consumed) || !count(data.autonomous_round_budget)) return;
    return { summary: data.objective, items: [{ title: data.objective, fields: [
      { label: tx('tools:detail.state'), value: tx(data.phase === 'complete' ? 'tools:detail.goal-complete' : data.phase === 'active' ? 'agent:copy.active' : data.phase === 'paused' ? 'agent:copy.paused' : 'agent:copy.blocked') },
      { label: tx('tools:detail.rounds'), value: `${data.autonomous_rounds_consumed} / ${data.autonomous_round_budget}` },
      ...(typeof data.blocked_reason === 'string' ? [{ label: tx('tools:detail.reason'), value: data.blocked_reason }] : []),
    ] }] };
  }
  if (toolId === 'tool-todo') {
    // Native Todo publishes a human receipt followed by the complete post-call snapshot.
    if (blocks.length !== 2 || blocks[0]?.type !== 'text' || blocks[1]?.type !== 'json') return;
    const snapshot = object(blocks[1].value);
    if (!Array.isArray(snapshot?.tasks)) return;
    const items: ToolDetailsModel['items'][number][] = [];
    for (const entry of snapshot.tasks) {
      const task = object(entry);
      if (!task || !count(task.id) || typeof task.subject !== 'string' || !['pending', 'in_progress', 'completed', 'deleted'].includes(String(task.status))) return;
      items.push({ title: task.subject, subtitle: `#${task.id}`,
        status: { value: task.status as 'pending' | 'in_progress' | 'completed' | 'deleted', label: tx(task.status === 'pending' ? 'tools:detail.todo-pending' : task.status === 'in_progress' ? 'tools:detail.todo-progress' : task.status === 'completed' ? 'tools:detail.todo-complete' : 'tools:detail.todo-deleted') },
        description: typeof task.description === 'string' ? task.description : undefined,
        fields: [
          ...(typeof task.owner === 'string' ? [{ label: tx('tools:detail.owner'), value: task.owner }] : []),
          ...(Array.isArray(task.blocked_by) && task.blocked_by.length ? [{ label: tx('tools:detail.dependencies'), value: task.blocked_by.map(id => `#${id}`).join(', ') }] : []),
        ],
      });
    }
    return { items, summary: blocks[0].text, empty: tx('tools:detail.todo-empty') };
  }
}
