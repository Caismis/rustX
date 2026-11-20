import type { ForegroundToolExecution } from '../../../protocol/app-server/v80';
import type { ToolCardView } from '../presentation/agent/ToolCard';
import { json } from './projection';
import { bashOutput, bashPromotion, launchResult } from './jobs';
import { parseModelToolCall } from '../../../protocol/app-server/model-tool-call';
export const READ_IMAGE_TOOL = 'tool-read-image';
export const READ_IMAGE_RECEIPT = 'Read static PNG image';
const variants: Record<string, ToolCardView['variant']> = {
  'tool-bash': 'bash',
  'tool-read': 'read',
  [READ_IMAGE_TOOL]: 'image',
  'tool-write': 'write',
  'tool-edit': 'edit',
  'tool-glob': 'search',
  'tool-grep': 'search',
  'tool-web-search': 'web',
  'tool-web-fetch': 'web',
  'tool-run-code': 'code',
};

function safeHttpUrl(value: string | undefined): string | undefined {
  if (!value) return undefined;
  try {
    const parsed = new URL(value);
    return parsed.protocol === 'http:' || parsed.protocol === 'https:' ? value : undefined;
  } catch {
    return undefined;
  }
}

export function readImageName(path?: string) {
  return path?.split(/[/\\]/).pop() || undefined;
}
/** Finite formatting of one native lifecycle. Never joins call/result messages. */
export function toolCard(tool: ForegroundToolExecution): ToolCardView {
  const variant = variants[tool.tool_id ?? ''] ?? (tool.name === 'web_search' || tool.name === 'web_fetch' ? 'web' : 'generic');
  const envelope = parseModelToolCall(tool.state.arguments);
  const args = envelope?.arguments ?? {};
  const formattedInput = tool.state.arguments;
  const result = tool.state.type === 'settled' ? tool.state.result : undefined;
  // C owns the displayed body; J alone owns execution settlement. A late C
  // changes the body without rewriting a running or already settled state.
  const body = tool.committed_result ?? result;
  const content = body?.content ?? [];
  const promotion = variant === 'bash' && body?.status.type === 'success' && body.exit_code == null && content.length === 1 && content[0]?.type === 'text'
    ? bashPromotion(content[0].text) : undefined;
  const launch = promotion ?? launchResult(body, tool.name);
  const status = result?.status.type;
  const background = (status === undefined || status === 'success') && (launch !== undefined || tool.state.type !== 'settled' && envelope?.execution_mode === 'background');
  const state = status === 'outcome_unknown' ? 'uncertain' : status === 'success' ? 'success' : status === 'cancelled' ? 'cancelled' : status ? 'failure' : tool.state.type === 'running' ? 'running' : 'assembled';
  const diagnostic = result?.status ?? body?.status;
  const detail = diagnostic && 'detail' in diagnostic ? diagnostic.detail : diagnostic && 'error' in diagnostic ? diagnostic.error : diagnostic && 'reason' in diagnostic ? diagnostic.reason : undefined;
  const string = (key: string) => typeof args[key] === 'string' ? args[key] as string : undefined;
  const positiveInt = (key: string) => typeof args[key] === 'number' && Number.isInteger(args[key]) && (args[key] as number) >= 1 ? args[key] as number : undefined;
  const imageResult = variant === 'image' && body?.status.type === 'success' && content.some(block => block.type === 'image');
  const visibleContent = imageResult ? content.filter(block => block.type !== 'text' || block.text !== READ_IMAGE_RECEIPT) : content;
  const output = launch ? promotion?.output : body ? visibleContent.flatMap(block => block.type === 'text' ? [block.text] : block.type === 'json' ? [(variant === 'bash' ? bashOutput(block.value) : undefined) ?? json(block.value)] : []).join('\n') : tool.state.type === 'running' && tool.state.progress ? json(tool.state.progress) : undefined;
  const edits = variant === 'edit' && Array.isArray(args.edits) ? args.edits.filter((edit): edit is { oldText: string; newText: string } => !!edit && typeof edit.oldText === 'string' && typeof edit.newText === 'string') : [];
  const path = string('path');
  const filePathLine = variant === 'read' ? positiveInt('offset') ?? positiveInt('line') : undefined;
  const href = variant === 'web' ? safeHttpUrl(string('url')) : undefined;
  const summary = variant === 'bash'
    ? string('description')?.split('\n')[0] ?? ''
    : detail ?? (variant === 'web' ? string('query') ?? string('url') : string(variant === 'search' ? 'pattern' : 'path')) ?? '';
  return {
    id: tool.call_id ?? undefined, background, jobId: launch?.jobId, promotion, path, filePathLine, href, imageResult, exitCode: result?.exit_code, truncated: result?.truncation?.truncated,
    diffs: variant === 'edit' ? edits.map(edit => ({ path: path ?? '', oldText: edit.oldText, newText: edit.newText })) : variant === 'write' && string('content') !== undefined ? [{ path: path ?? '', oldText: null, newText: string('content')! }] : undefined,
    nativeName: tool.name, title: tool.name, variant, state, summary,
    input: variant === 'bash' ? string('command') ?? tool.state.arguments : variant === 'code' ? string('code') ?? tool.state.arguments : variant === 'image' ? undefined : formattedInput,
    output: [detail, output].filter(Boolean).join('\n') || undefined,
  };
}
