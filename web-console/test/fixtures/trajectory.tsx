import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Trajectory } from '../../src/app/trajectory/Trajectory';
import { completeTraceDetail, prependTrace, refreshTrace, replaceTrace, selectTrace } from '../../src/client/trace';
import { structuralSearchRecords, traceRecord, traceTool, requestDetail, toolDetail } from '../trace-fixture';
import type { TraceRecord } from '../../../protocol/app-server/v23';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/reset.css';
const text = (text: string) => ({ text, truncated: false });
function request(n: number, prompt: 'initial' | 'changed' | 'unchanged', tools: 'initial' | 'changed' | 'unchanged', state: TraceRecord['state'] = 'completed') {
  const record = traceRecord(n, { state });
  record.request!.model = 'deepseek-chat';
  record.request!.system_prompt = { state: prompt, preview: prompt === 'unchanged' ? null : text('You are the historical agent. Preserve exact native authority.') };
  record.request!.tool_catalog = tools;
  record.request!.retry_number = n === 3 ? 0 : 1;
  if (n === 3) record.request!.context_additions = [
    { message_id: 'context-workspace', context_kind: 'native_environment', producer: { Native: 'workspace_instructions' }, source: { type: 'runtime' }, preview: text('Workspace /project · use the checked-in Rust conventions.'), attachments: [], truncated: false },
    { message_id: 'context-agent', context_kind: 'agent_status', producer: { Native: 'agent_status' }, source: { type: 'runtime' }, preview: text('Review the implementation and report failing checks.'), attachments: [], truncated: false },
  ];
  return record;
}
const records = [
  traceRecord(0, { kind: 'user', request: null, location: {}, preview: text('Inspect **the workspace** and summarize what changed.') }),
  traceRecord(1, { kind: 'attempt', request: null, location: { attempt_id: 'attempt-a' }, preview: null }),
  traceRecord(14, { kind: 'user', request: null, location: { attempt_id: 'attempt-a' }, preview: text('Use the native Trace facts for this review.') }),
  traceRecord(2, { kind: 'step', request: null, preview: null }),
  request(3, 'initial', 'initial'),
  traceRecord(4, { kind: 'assistant', request: null, message_id: 'assistant-4', preview: text('I’ll inspect the **working tree**.'), calls: [{ call_id: 'call-5', tool_id: 'tool-bash', name: 'bash' }, { call_id: 'proposed-only', tool_id: 'tool-read', name: 'read' }] }),
  traceTool(5, { state: 'failed', preview: text('git diff --stat'), tool: { ...traceTool(5).tool!, outcome: 'failed', detail: text('Missing field `name`') } }),
  traceRecord(6, { kind: 'background', request: null, native_id: 'background-6', preview: text('Indexing workspace'), state: 'running', timing: { started_at: '2026-09-15T00:00:02Z' } }),
  request(7, 'changed', 'unchanged'), request(8, 'unchanged', 'changed'), request(9, 'changed', 'changed', 'failed'),
  traceRecord(10, { kind: 'compaction', request: null, location: {}, state: 'running', preview: text('Summarizing earlier messages…') }),
  request(11, 'changed', 'unchanged', 'failed'),
  traceRecord(12, { kind: 'subagent', request: null, location: {}, preview: text('Delegated review of the parser') }),
  traceRecord(13, { kind: 'workflow', request: null, location: {}, preview: text('Release checklist run') }),
];
const params = new URLSearchParams(location.search);
const long = params.has('long'); const threshold = params.has('threshold');
const renumber = params.has('renumber');
const structure = params.has('structure'); const toolFirst = params.has('tool');
const snapshot = { records: long || threshold ? Array.from({ length: long ? 480 : 90 }, (_, n) => toolFirst && n === 0 ? traceTool(100) : request(n + 100, !threshold && n % 7 === 0 ? 'changed' : 'unchanged', 'unchanged')) : params.has('structural-search') ? structuralSearchRecords() : records, next_cursor: 'older' };
function Fixture() {
  const [cache, setCache] = useState(() => replaceTrace(snapshot));
  const [reads, setReads] = useState(0); const [pages, setPages] = useState(0);
  return <main style={{ height: '100dvh', display: 'flex', flexDirection: 'column' }}>
    <header style={{ padding: '8px 12px', display: 'flex', gap: 8, borderBottom: '1px solid var(--dsw-alias-border-l2)', fontSize: 11 }}><strong>rustX / Workspace review</strong>
      <button onClick={() => setCache(current => refreshTrace(current, { records: [...current.page.records, traceRecord(Number(current.page.records.at(-1)!.id.split(':')[1]) + 1)], next_cursor: current.page.next_cursor }))}>Append</button>
      <button onClick={() => setCache(current => ({ ...current, page: { ...current.page, records: current.page.records.map(record => ({ ...record, preview: text('Lifecycle updated') })) } }))}>Update</button>
      <span data-detail-reads={reads} data-history-reads={pages} data-native-count={cache.page.records.length} data-trace-epoch={cache.epoch}>Fixture</span>
    </header>
    <Trajectory cache={cache} onSelect={id => setCache(current => selectTrace(current, id))}
      latest={() => setCache(current => replaceTrace(snapshot, current))}
      loadEarlier={() => { setPages(n => n + 1); setCache(current => prependTrace(current, { records: renumber ? [traceRecord(50, { location: { attempt_id: 'older-attempt', step_id: 'old-step' } })] : Array.from({ length: 32 }, (_, n) => structure && n < 2 ? traceRecord(n + 50, { kind: n === 0 ? 'attempt' : 'step', request: null, location: n === 0 ? { attempt_id: 'attempt-a' } : { attempt_id: 'attempt-a', step_id: '1' } }) : request(n + 50, 'changed', 'unchanged')), next_cursor: null })); }}
      onLoadDetail={id => {
        setReads(n => n + 1);
        setCache(current => {
          const n = Number(id.split(':')[1]); const record = current.page.records.find(record => record.id === id)!;
          const detail = record.kind === 'tool' ? toolDetail(n) : record.kind === 'request' ? requestDetail(n) : requestDetail(n, { kind: record.kind, request: null, messages: [] });
          if (detail.request) {
            detail.request.messages.push(...(record.request?.context_additions ?? []).map(context => ({ role: 'user' as const, message_id: context.message_id, source: 'runtime', blocks: [{ type: 'text' as const, text: text(`Full content: ${context.preview?.text}`) }], truncated: false })));
            if (n === 3) { detail.request.predecessor = { availability: 'not_applicable' }; detail.request.previous_system_prompt = null; }
            if (n === 11) { detail.request.effective_system_prompt.truncated = true; detail.request.previous_system_prompt!.truncated = true; }
          }
          if (detail.tool) {
            detail.tool.source!.text = text('git diff --stat');
            detail.tool.result!.blocks = [{ type: 'json', value: { value: { files: 3, insertions: 28 }, truncated: false } }, { type: 'text', text: text('Review complete.') }];
          }
          return completeTraceDetail(current, id, current.epoch, detail);
        });
      }} />
  </main>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
