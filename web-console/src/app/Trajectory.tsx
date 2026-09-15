/* Copyright (c) 2026 DeepSeek. MIT. Rewritten from ui-trajectory; see PROVENANCE.md. */
import { useCallback, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import type { TraceEntry, TraceKind, TraceText } from '../../../protocol/app-server/v3';
import type { TraceCache } from '../client/trace';
import { TRACE_LIMIT } from '../client/trace';
import { Button } from '../presentation/primitives/Button';
import { Input } from '../presentation/primitives/Input';
import { Artifact } from './components/Artifact';
import css from './Trajectory.module.css';

const KINDS: TraceKind[] = ['attempt', 'step', 'request', 'assistant', 'tool', 'compaction', 'background', 'subagent', 'workflow', 'interaction'];
function text(value: TraceText) { return value.redacted ? 'Redacted · internal request or execution content' : value.text; }
function Payload({ value }: { value: TraceText }) {
  return <div><pre>{text(value)}</pre>{value.truncated && <small>Content shown partially · truncated</small>}</div>;
}
function label(entry: TraceEntry) {
  return entry.kind === 'request' && entry.request ? `Request #${entry.request.retry_number} · ${entry.request.model.text}`
    : entry.kind === 'step' ? `Step ${entry.location.step_id ?? 'unavailable'}`
      : entry.kind === 'attempt' ? `Turn / Attempt ${entry.location.attempt_id ?? 'unavailable'}`
        : `${entry.kind} · ${entry.tool?.call_id ?? entry.native_id ?? entry.message_id ?? entry.id}`;
}
function stepGroup(entry: TraceEntry) { return JSON.stringify([entry.location.attempt_id, entry.location.step_id]); }
function duration(entry: TraceEntry) { return entry.timing.duration_ms == null ? 'Unavailable' : `${entry.timing.duration_ms} ms`; }
/** Search and folding use only the loaded typed Trace window. */
export function visibleTrace(entries: TraceEntry[], query: string, kind: string, folded: ReadonlySet<string>) {
  const needle = query.toLocaleLowerCase();
  return entries.filter(entry => {
    const group = entry.location.attempt_id;
    if (!needle && group && folded.has(group) && entry.kind !== 'attempt') return false;
    if (!needle && entry.location.step_id && folded.has(stepGroup(entry)) && !['attempt', 'step'].includes(entry.kind)) return false;
    if (kind && entry.kind !== kind) return false;
    return !needle || [label(entry), entry.id, entry.request?.request_id, entry.tool?.tool_id, entry.native_id, entry.location.attempt_id, entry.state, entry.location.step_id, ...entry.output.map(text), ...entry.reasoning.map(text)].join(' ').toLocaleLowerCase().includes(needle);
  });
}
function Overview({ entries, select }: { entries: TraceEntry[]; select: (id: string) => void }) {
  const timed = entries.filter(entry => ['request', 'tool', 'compaction', 'subagent', 'workflow'].includes(entry.kind));
  const starts = timed.map(entry => Date.parse(entry.timing.started_at));
  const ends = timed.map(entry => Date.parse(entry.timing.ended_at ?? entry.timing.started_at));
  const start = starts.length ? Math.min(...starts) : 0;
  const span = Math.max(1, ...(ends.map(end => end - start)));
  return <section className={css.overview} aria-label="Timing Overview"><strong>Overview</strong><small>Recorded timing · loaded window only</small>
    {!timed.length && <p>Timing unavailable</p>}
    {(['request', 'tool', 'compaction', 'subagent', 'workflow'] as const).map(kind => <div className={css.lane} key={kind}><span>{kind === 'request' ? 'Model' : kind}</span><div>
      {timed.filter(entry => entry.kind === kind).map(entry => <button key={entry.id} aria-label={`Inspect timing ${label(entry)}`} title={`${label(entry)} · ${duration(entry)}`}
        className={css.span} onClick={() => select(entry.id)} style={{ left: `${(Date.parse(entry.timing.started_at) - start) / span * 100}%`, width: entry.timing.duration_ms == null ? '2px' : `${Math.max(.3, Number(entry.timing.duration_ms) / span * 100)}%` }} />)}
    </div></div>)}
  </section>;
}
function Inspector({ entry, close }: { entry: TraceEntry; close: () => void }) {
  const [tab, setTab] = useState('Summary');
  const tabs = ['Summary', ...(entry.request || entry.tool ? ['Input'] : []), ...(entry.output.length || entry.reasoning.length ? ['Output'] : []), ...(entry.request ? ['Schema', 'Usage'] : []), 'Timing', ...(entry.artifacts.length ? ['Attachments'] : [])];
  const active = tabs.includes(tab) ? tab : 'Summary';
  return <aside className={css.inspector} aria-label="Trace record inspector"><header><strong>{label(entry)}</strong><Button size="sm" onClick={close}>Close record</Button></header>
    <div role="tablist" aria-label="Record sections">{tabs.map(name => <Button size="sm" key={name} role="tab" aria-selected={active === name} onClick={() => setTab(name)}>{name}</Button>)}</div>
    <div role="tabpanel">
      {active === 'Summary' && <dl><dt>State</dt><dd>{entry.state}</dd><dt>Trace identity</dt><dd>{entry.id}</dd><dt>Attempt</dt><dd>{entry.location.attempt_id ?? 'Unavailable'}</dd><dt>Logical Step</dt><dd>{entry.location.step_id ?? 'Unavailable'}</dd>
        {entry.request && <><dt>Actual request</dt><dd>{entry.request.request_id}</dd><dt>Request ordinal</dt><dd>{entry.request.retry_number} {entry.request.retry_number > 0 ? '· retry / recovery within this Step' : '· initial'}</dd><dt>Previous request failure</dt><dd>{entry.request.previous_failure_kind ?? 'Unavailable'}</dd><dt>Failure class</dt><dd>{entry.request.failure_kind ?? 'None recorded'}</dd></>}
        {entry.tool && <><dt>ToolCall</dt><dd>{entry.tool.call_id}</dd><dt>Tool</dt><dd>{entry.tool.tool_id}</dd></>}
        {entry.calls.map(call => <div key={call.call_id}><dt>Assembled canonical ToolCall</dt><dd>{call.call_id} · {call.tool_id} · execution requires its own evidence</dd></div>)}
        {entry.native_id && <><dt>Native identity</dt><dd>{entry.native_id}</dd></>}
        {entry.message_id && <><dt>Canonical message</dt><dd>{entry.message_id}</dd></>}
        {entry.kind === 'request' && <p>Provider completion alone does not prove canonical Assistant acceptance.</p>}
        {entry.truncated && <p>Content shown partially · truncated</p>}
      </dl>}
      {active === 'Input' && <>{entry.request && <><h3>Effective System Prompt</h3><Payload value={entry.request.effective_system_prompt} /><h3>Request context</h3><Payload value={entry.request.context_input} /><h3>Historical model</h3><Payload value={entry.request.model} /><p>Output token limit: {entry.request.max_output_tokens}</p><p>Reasoning: {entry.request.reasoning_enabled ? 'enabled' : 'disabled'}</p></>}{entry.tool && <Payload value={entry.tool.arguments} />}</>}
      {active === 'Output' && <>{entry.reasoning.map((value, index) => <details key={`reasoning:${index}`}><summary>Reasoning</summary><Payload value={value} /></details>)}{entry.output.map((value, index) => <Payload key={index} value={value} />)}</>}
      {active === 'Schema' && entry.request && <Payload value={entry.request.tool_schema} />}
      {active === 'Timing' && <dl><dt>Started</dt><dd>{entry.timing.started_at}</dd><dt>Ended</dt><dd>{entry.timing.ended_at ?? 'Unavailable'}</dd><dt>Duration</dt><dd>{duration(entry)}</dd></dl>}
      {active === 'Usage' && (entry.request?.usage ? <dl><dt>Input tokens</dt><dd>{entry.request.usage.input_tokens}</dd><dt>Output tokens</dt><dd>{entry.request.usage.output_tokens}</dd><dt>Total tokens</dt><dd>{entry.request.usage.total_tokens}</dd><dt>Reasoning tokens</dt><dd>{entry.request.usage.details?.reasoning_tokens ?? 'Unavailable'}</dd><dt>Cached input</dt><dd>{entry.request.usage.details?.cached_input_tokens ?? 'Unavailable'}</dd></dl> : <p>Usage unavailable</p>)}
      {active === 'Attachments' && entry.artifacts.map(reference => <Artifact key={reference.artifact_id} id={reference.artifact_id} image={reference.image} />)}
    </div>
  </aside>;
}
export function Trajectory({ cache, loadEarlier, latest }: { cache: TraceCache; loadEarlier: () => void; latest: () => void }) {
  const [query, setQuery] = useState('');
  const [kind, setKind] = useState('');
  const [folded, setFolded] = useState<Set<string>>(new Set());
  const [selectedId, setSelectedId] = useState<string>();
  const entries = cache.page.entries;
  const rows = useMemo(() => visibleTrace(entries, query, kind, folded), [entries, query, kind, folded]);
  const selected = entries.find(entry => entry.id === selectedId);
  const viewport = useRef<HTMLDivElement>(null);
  const key = useCallback((index: number) => rows[index]!.id, [rows]);
  const virtualizer = useVirtualizer({ count: rows.length, getScrollElement: () => viewport.current,
    estimateSize: () => 36, getItemKey: key, overscan: 8, initialRect: { width: 800, height: 500 },
    anchorTo: 'end', followOnAppend: 'auto', scrollEndThreshold: 2 });
  const mounted = useRef(false);
  useLayoutEffect(() => { if (!mounted.current && rows.length) { mounted.current = true; virtualizer.scrollToIndex(rows.length - 1, { align: 'end' }); } }, [rows.length, virtualizer]);
  // A cache replacement never silently moves selection to another record.
  const removed = selectedId && !selected;
  return <section className={css.root} aria-label="Trajectory"><div className={css.toolbar}>
    <Input aria-label="Search loaded Trace" value={query} onChange={event => setQuery(event.target.value)} placeholder="Search loaded Trace" />
    <select aria-label="Trace category" value={kind} onChange={event => setKind(event.target.value)}><option value="">All records</option>{KINDS.map(value => <option key={value}>{value}</option>)}</select>
    <Button size="sm" disabled={cache.loading || !cache.page.next_cursor || entries.length >= TRACE_LIMIT} onClick={loadEarlier}>{cache.loading ? 'Loading Trace…' : 'Load older Trace'}</Button>
    <Button size="sm" onClick={latest}>Latest Trace</Button><small>{entries.length} loaded</small>
  </div>{cache.error && <p role="alert">{cache.error}</p>}
    <Overview entries={entries} select={setSelectedId} />
    {removed && <p role="status">Selected record left the loaded window. <Button size="sm" onClick={() => setSelectedId(undefined)}>Dismiss selection</Button></p>}
    <div className={css.split}><div ref={viewport} className={css.ledger} role="table" aria-label="Trace ledger" aria-rowcount={rows.length} style={{ overflowAnchor: 'none' }}>
      <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
        {virtualizer.getVirtualItems().map(item => { const entry = rows[item.index]!; const group = entry.kind === 'step' ? stepGroup(entry) : entry.location.attempt_id; return <div key={item.key} data-trace-id={entry.id} role="row" aria-rowindex={item.index + 1} aria-selected={selectedId === entry.id} className={css.record} style={{ position: 'absolute', top: 0, transform: `translateY(${item.start}px)`, height: item.size, width: '100%' }}>
          <span role="cell" className={css.group}>{['attempt', 'step'].includes(entry.kind) && group ? <Button size="sm" aria-label={`Fold ${entry.kind === 'step' ? 'Step ' + entry.location.step_id : 'Attempt ' + group}`} aria-expanded={!folded.has(group)} onClick={() => setFolded(current => { const next = new Set(current); if (next.has(group)) next.delete(group); else next.add(group); return next; })}>{folded.has(group) ? '▸' : '▾'}</Button> : null}{entry.location.step_id ? `Step ${entry.location.step_id}` : entry.kind}</span>
          <button role="cell" className={css.recordButton} onClick={() => setSelectedId(entry.id)} title={label(entry)}>{label(entry)}</button><span role="cell">{entry.state}</span><span role="cell">{duration(entry)}</span>
        </div>; })}
      </div>{!rows.length && <p>No matching loaded records.</p>}
    </div>{selected && <Inspector key={selected.id} entry={selected} close={() => setSelectedId(undefined)} />}</div>
  </section>;
}
