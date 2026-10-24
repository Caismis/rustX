import { useEffect, useRef, useState } from 'react';
import type { CheckpointPage, TraceArtifact, TraceContentBlock, TraceInspectionHead, TraceToolDefinition, TraceToolResult, TraceToolSource, TraceText, UserSource } from '../../../../protocol/app-server/v57';
import { traceFields } from '../../../../protocol/app-server/trace-identity';
import { readRequestField, readTraceBlocks, readTraceMessages, readTraceSource, traceProgram } from '../../client/trace-content';
import type { TraceFieldReader } from '../../client/trace';
import type { InspectableDisplayItem, TrajectoryFacet } from './layout';

export type ToolContent = { name?: string; arguments?: { value: unknown; truncated: boolean }; source?: TraceToolSource; definition?: TraceToolDefinition; result?: Pick<TraceToolResult, 'outcome' | 'detail' | 'blocks' | 'attachments'> | null };
export type RequestContent = { options?: Record<string, unknown>; effective_system_prompt?: TraceText; previous_system_prompt?: TraceText; tools?: TraceToolDefinition[] };
interface Content { blocks?: TraceContentBlock[]; source?: UserSource | null; attachments?: TraceArtifact[]; tool?: ToolContent; request?: RequestContent }
const object = (value: unknown): Record<string, unknown> => value != null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {};
function prompt(value: unknown): TraceText {
  const source = object(value);
  if (typeof source.text !== 'string' || typeof source.truncated !== 'boolean') throw Error('Original system prompt is unavailable.');
  return {text:source.text,truncated:source.truncated};
}
const optionFields = ['model', 'protocol', 'reasoning_enabled', 'profile', 'max_output_tokens', 'context_window_tokens', 'options'] as const;

/** Transport adapters feed the same semantic views used by the Harness-aligned UI. */
export function useTraceContent(page: CheckpointPage | undefined, reader: TraceFieldReader | undefined, inspection: TraceInspectionHead | undefined, item: InspectableDisplayItem, facet: TrajectoryFacet, previousDomain?: string) {
  const section = inspection?.content.type === 'tool' ? 'tool' : inspection?.content.type === 'messages' ? facet === 'source' ? 'source' : 'message' : item.record.kind === 'request' && (facet === 'overview' || facet === 'options') ? 'options' : facet;
  const identity = JSON.stringify([page, item.type, item.type === 'ContextRow' ? item.context.locator : undefined, section, previousDomain]);
  const owner = JSON.stringify([page?.read_cut.conversation_id, page?.read_cut.activation_id,
    item.owner_record_id, item.type, item.type === 'ContextRow' ? item.context.locator : undefined, section, previousDomain]);
  const latest = useRef({ identity, reader }); latest.current = { identity, reader };
  const [state, setState] = useState<{ identity: string; owner: string; reader: typeof reader; content?: Content; error?: string }>();
  const loaded = state?.identity === identity && state.reader === reader ? state : undefined;
  // A new cut for the same selected content is read behind the resident view.
  // Facet/record/source changes and missing native headers never borrow a body.
  const displayed = loaded ?? (page && state?.owner === owner ? state : undefined);
  useEffect(() => {
    if (!page || !reader) return;
    let mounted = true;
    const current = () => mounted && latest.current.identity === identity && latest.current.reader === reader;
    void (async (): Promise<Content> => {
      const cut = page.read_cut;
      if (inspection?.content.type === 'messages') {
        if (facet === 'source') return { source: inspection.content.page.items[0]?.source ?? undefined };
        return readTraceMessages(reader, cut, inspection.content.page, current);
      }
      if (inspection?.content.type === 'tool') {
        const record = inspection.record, fields = traceFields(record), tool: ToolContent = {};
        const nameField = fields.find(field => field.field === 'name'), idField = fields.find(field => field.field === 'tool_id');
        const name = record.tool?.name ?? (nameField ? await readTraceSource(reader, cut, nameField, current) : undefined);
        const toolId = record.tool?.tool_id ?? (idField ? await readTraceSource(reader, cut, idField, current) : undefined);
        if (typeof name === 'string') tool.name = name;
        const argumentsField = fields.find(field => field.field === 'arguments');
        if (argumentsField) {
          tool.arguments = { value: await readTraceSource(reader, cut, argumentsField, current), truncated: false };
          tool.source = traceProgram(typeof toolId === 'string' ? toolId : undefined, tool.arguments.value);
        }
        if (inspection.content.definition.type === 'available') tool.definition = await readTraceSource(reader, cut, inspection.content.definition.field, current) as TraceToolDefinition;
        const blocks = await readTraceBlocks(reader, cut, record.collections, current);
        const resultField = fields.find(field => field.field === 'result');
        const metadata = resultField ? object(await readTraceSource(reader, cut, resultField, current)) : undefined;
        if (blocks !== undefined || metadata) {
          const status = object(metadata?.status), outcome = record.tool?.outcome ?? status.type;
          if (typeof outcome !== 'string' || !['success', 'failed', 'denied', 'cancelled', 'timed_out', 'outcome_unknown'].includes(outcome)) throw Error('Original Tool outcome is unavailable.');
          const detail = status.type === 'failed' ? status.error : status.type === 'denied' ? status.reason : status.type === 'outcome_unknown' ? status.detail : undefined;
          tool.result = { outcome: outcome as TraceToolResult['outcome'], blocks: blocks ?? [], attachments: record.attachments, detail: typeof detail === 'string' ? {text:detail,truncated:false} : record.tool?.detail };
        }
        return { tool };
      }
      if (item.type === 'ContextRow' && item.context.locator.type === 'by_ledger_position') {
        if (facet === 'source') return {};
        const locator = item.context.locator;
        const domain = `messages.ledger:${locator.position}.blocks`;
        const first = await reader.page(cut, domain, 0, current);
        // The selected native locator fixes its blocks extent before the bounded reads.
        const blocks = await readTraceBlocks(reader, cut, [{ domain, total: first.total, revision: '1', loaded: '0' }], current, first, locator);
        return { blocks, attachments: item.context.attachments };
      }
      if (item.record.kind !== 'request') return {};
      const request: RequestContent = {}, domain = `${page.domain}.request`;
      if (facet === 'overview' || facet === 'options') {
        const values: Record<string, unknown> = {};
        for (const field of optionFields) values[field] = await readRequestField(reader, cut, `${domain}.${field}`, current);
        request.options = { model: values.model, protocol: values.protocol, thinking: values.reasoning_enabled ? 'enabled' : 'disabled', ...(values.profile ? { profile: values.profile } : {}), maxTokens: values.max_output_tokens, contextWindowTokens: values.context_window_tokens,
          ...Object.fromEntries((values.options as { name: string; value: { value: unknown } }[]).map(option => [option.name, option.value.value])) };
      }
      if (facet === 'diff' && item.record.request?.predecessor.availability === 'available' && !previousDomain) return { request };
      if (facet === 'system-prompt' || facet === 'diff') request.effective_system_prompt = prompt(await readRequestField(reader, cut, `${domain}.effective_system_prompt`, current));
      if (facet === 'diff' && previousDomain) request.previous_system_prompt = prompt(await readRequestField(reader, cut, previousDomain, current));
      if (facet === 'tools') {
        const tools = await readRequestField(reader, cut, `${domain}.tools`, current);
        if (!Array.isArray(tools)) throw Error('Original Tool catalog is unavailable.');
        request.tools = tools as TraceToolDefinition[];
      }
      return { request };
    })().then(content => { if (current()) setState({ identity, owner, reader, content }); })
      .catch(error => { if (current()) setState(previous => ({ identity, owner, reader,
        content: previous?.owner === owner ? previous.content : undefined, error: String(error).slice(0, 512) })); });
    return () => { mounted = false; };
  }, [identity, reader]);
  return displayed;
}
