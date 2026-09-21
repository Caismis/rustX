import type { ForegroundToolExecution } from '../../../protocol/app-server/v16';
import type { ToolCardView } from '../presentation/agent/ToolCard';
import { json } from './projection';
const variants: Record<string, ToolCardView['variant']> = { 'tool-bash': 'bash', 'tool-read': 'read', 'tool-write': 'write', 'tool-edit': 'edit', 'tool-glob': 'search', 'tool-grep': 'search' };
/** Finite formatting of one native lifecycle. Never joins call/result messages. */
export function toolCard(tool: ForegroundToolExecution): ToolCardView {
  const variant = variants[tool.tool_id] ?? 'generic';
  let args: Record<string, unknown> = {};
  try { const value = JSON.parse(tool.state.arguments); if (value && typeof value === 'object' && !Array.isArray(value)) args = value; } catch { /* A streaming argument fragment is shown verbatim. */ }
  const result = tool.state.type === 'settled' ? tool.state.result : undefined;
  const status = result?.status.type;
  const state = status === 'outcome_unknown' ? 'uncertain' : status === 'success' ? 'success' : status === 'cancelled' ? 'cancelled' : status ? 'failure' : tool.state.type === 'running' ? 'running' : 'assembled';
  const detail = result?.status && 'detail' in result.status ? result.status.detail : result?.status && 'error' in result.status ? result.status.error : result?.status && 'reason' in result.status ? result.status.reason : undefined;
  const string = (key: string) => typeof args[key] === 'string' ? args[key] as string : undefined;
  const output = result ? (result.content ?? []).flatMap(block => block.type === 'text' ? [block.text] : block.type === 'json' ? [json(block.value)] : []).join('\n') : tool.state.type === 'running' && tool.state.progress ? json(tool.state.progress) : undefined;
  const edits = variant === 'edit' && Array.isArray(args.edits) ? args.edits.filter((edit): edit is { oldText: string; newText: string } => !!edit && typeof edit.oldText === 'string' && typeof edit.newText === 'string') : [];
  return { id: tool.call_id, nativeName: tool.name, title: tool.name, variant, state,
    summary: detail ?? string(variant === 'bash' ? 'command' : variant === 'search' ? 'pattern' : 'path') ?? tool.call_id,
    input: variant === 'bash' ? string('command') ?? tool.state.arguments : tool.state.arguments,
    output: [detail, output].filter(Boolean).join('\n') || undefined,
    removed: edits.length ? edits.map(edit => edit.oldText).join('\n') : undefined,
    added: edits.length ? edits.map(edit => edit.newText).join('\n') : variant === 'write' ? string('content') : undefined };
}
