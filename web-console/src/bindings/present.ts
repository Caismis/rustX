import type { ForegroundToolExecution, SessionFileReference, ToolExecutionResult } from '../../../protocol/app-server/v44';
import type { PresentRowView } from '../presentation/agent/PresentRow';

/** Raw arguments can be partial while a call streams; then they show verbatim. */
function declaredPaths(raw: string): string {
  let args: unknown;
  try { args = JSON.parse(raw); } catch { return raw; }
  if (typeof args !== 'object' || args === null || !('files' in args) || !Array.isArray(args.files)) return raw;
  return args.files.flatMap((file: unknown) =>
    typeof file === 'object' && file !== null && 'path' in file && typeof file.path === 'string' ? [file.path] : []).join(', ');
}

/** The Harness present phase of one native lifecycle. Output text decides nothing. */
export function presentRow(tool: ForegroundToolExecution): PresentRowView {
  const result = tool.state.type === 'settled' ? tool.state.result : undefined;
  const status = result?.status;
  const phase = !status ? (tool.state.type === 'running' ? 'running' : 'preparing')
    : status.type === 'success' ? 'ok' : status.type === 'cancelled' ? 'stopped' : 'error';
  const reason = status && 'error' in status ? status.error : status && 'reason' in status ? status.reason : status && 'detail' in status ? status.detail : undefined;
  const output = (result?.content ?? []).flatMap(block => block.type === 'text' ? [block.text] : []).join('\n');
  return { id: tool.call_id, phase, paths: declaredPaths(tool.state.arguments), details: [reason, output].filter(Boolean).join('\n') };
}

/** One committed delivery and its stable native address. */
export interface PresentedDelivery { readonly key: string; readonly messageId: string; readonly index: number; readonly file: SessionFileReference }

/**
 * The typed deliveries of one committed canonical Tool-result message, in
 * canonical order. Only a successful result delivers; arguments, content JSON
 * and prose are never read.
 */
export function presentedDeliveries(messageId: string, result: ToolExecutionResult): PresentedDelivery[] {
  if (result.status.type !== 'success') return [];
  return (result.deliveries ?? []).map((file, index) => ({ key: `${messageId}:${index}`, messageId, index, file }));
}
