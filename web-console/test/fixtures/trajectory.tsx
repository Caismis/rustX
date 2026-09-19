import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Trajectory } from '../../src/app/trajectory/Trajectory';
import { completeTraceDetail, prependTrace, refreshTrace, replaceTrace, selectTrace } from '../../src/client/trace';
import { traceRecord, traceTool, requestDetail, toolDetail } from '../trace-fixture';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/reset.css';

const text = (text: string) => ({ text, truncated: false });
const records = [
  traceRecord(0, { kind: 'user', request: null, location: {}, preview: text('Inspect **the workspace** and summarize what changed.'), attachments: [
    { artifact_id: 'brief-0', name: 'brief.md', mime_type: 'text/markdown', image: false },
    { artifact_id: 'shot-0', name: 'screenshot.png', mime_type: 'image/png', image: true },
  ] }),
  traceRecord(1, { kind: 'attempt', request: null, location: { attempt_id: 'attempt-a' }, preview: null }),
  traceRecord(2, { kind: 'step', request: null, preview: null }),
  traceRecord(3, { preview: text('deepseek-chat · historical request'), request: { ...traceRecord(3).request!, retry_number: 0 } }),
  traceRecord(4, { kind: 'assistant', request: null, preview: text('I’ll inspect the **working tree** and check `src/main.rs`.'), calls: [{ call_id: 'call-5', tool_id: 'tool-bash', name: 'bash' }] }),
  traceTool(5, { preview: text('git diff --stat'), tool: { ...traceTool(5).tool!, detail: text('3 files changed, 28 insertions') } }),
  traceRecord(6, { kind: 'background', request: null, native_id: 'background-6', preview: text('Indexing workspace'), state: 'running', timing: { started_at: '2026-09-15T00:00:02Z' } }),
  traceRecord(7, { kind: 'step', request: null, preview: null, location: { attempt_id: 'attempt-a', step_id: '2' } }),
  traceRecord(8, { location: { attempt_id: 'attempt-a', step_id: '2' }, preview: text('deepseek-chat · historical request'), request: { ...traceRecord(8).request!, retry_number: 1 } }),
  traceTool(9, { location: { attempt_id: 'attempt-a', step_id: '2' }, state: 'failed', preview: text('cargo check'), tool: { ...traceTool(9).tool!, outcome: 'failed', detail: text('Missing field `name`') } }),
  traceRecord(10, { kind: 'assistant', request: null, location: { attempt_id: 'attempt-a', step_id: '2' }, preview: text('The changes add a **workspace index**. One check needs attention.'), attachments: [
    { artifact_id: 'diagram-10', name: 'diagram.png', mime_type: 'image/png', image: true },
  ] }),
  traceRecord(11, { kind: 'compaction', request: null, location: {}, preview: text('Earlier messages summarized; workspace findings retained.') }),
  traceRecord(12, { kind: 'subagent', request: null, location: {}, preview: text('Delegated review of the parser') }),
  traceRecord(13, { kind: 'workflow', request: null, location: {}, preview: text('Release checklist run') }),
  traceRecord(14, { kind: 'interaction', request: null, location: {}, preview: text('Approval requested for write access') }),
];
const long = new URLSearchParams(location.search).has('long');
function Fixture() {
  const [cache, setCache] = useState(() => replaceTrace({ records: long ? Array.from({ length: 160 }, (_, n) => traceRecord(n + 100)) : records, next_cursor: 'older' }));
  return <main style={{ height: '100dvh', display: 'flex', flexDirection: 'column' }}>
    <header style={{ padding: '10px 16px', display: 'flex', justifyContent: 'space-between', borderBottom: '1px solid var(--dsw-alias-border-l2)' }}><strong>rustX / Workspace review</strong><button onClick={() => setCache(current => refreshTrace(current, { records: [...current.page.records, traceRecord(Number(current.page.records.at(-1)!.id.split(':')[1]) + 1)], next_cursor: current.page.next_cursor }))}>Append record</button></header>
    <Trajectory cache={cache} onSelect={id => setCache(current => selectTrace(current, id))}
      latest={() => setCache(replaceTrace({ records, next_cursor: null }))}
      loadEarlier={() => setCache(current => prependTrace(current, { records: Array.from({ length: 32 }, (_, n) => traceRecord(n + 50)), next_cursor: null }))}
      onLoadDetail={id => setCache(current => {
        const n = Number(id.split(':')[1]);
        const record = current.page.records.find(record => record.id === id)!;
        const detail = record.kind === 'tool' ? toolDetail(n) : record.kind === 'request' ? requestDetail(n) : requestDetail(n, { kind: record.kind, request: null, messages: [{ message_id: `message-${n}`, role: record.kind === 'user' ? 'user' : 'assistant', source: 'historical', blocks: [{ type: 'text', text: record.preview ?? text('') }], truncated: false }] });
        if (detail.tool) {
          detail.tool.arguments = { value: { command: 'git diff --stat', cwd: '/workspace/rustX' }, truncated: false };
          detail.tool.source!.text = text('git diff --stat');
          detail.tool.result!.blocks = [{ type: 'json', value: { value: { files: 3, insertions: 28 }, truncated: false } }, { type: 'text', text: text('**Review complete** · See the attached report.') }];
          detail.tool.result!.attachments = [{ artifact_id: 'report-5', name: 'review.md', mime_type: 'text/markdown', image: false }];
        }
        return completeTraceDetail(current, id, current.epoch, detail);
      })} />
  </main>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
