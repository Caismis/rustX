import type { Translate } from '../../locale/translation';
import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness ui-trajectory/TrajectoryTable.tsx inspector; see PROVENANCE.md. */
/**
 * The record inspector, ported from the Harness details panel.
 *
 * Tabs vary by record kind exactly as in Harness: a Summary of facts and
 * previews, then the full Preview/Raw, Payload/Result/Schema or
 * Options/Usage/Timing views. Values come from the native Trace record and
 * its fetched detail; where a fact is not recorded the panel says so with
 * Harness's own wording rather than inventing it.
 */
import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { structuredPatch } from 'diff';
import type { TraceArtifact, TraceContentBlock, TraceDetail, TraceJson, TraceRecord, TraceToolDefinition } from '../../../../protocol/app-server/v36';
import { writeClipboard } from '../../presentation/primitives/clipboard';
import { JsonTree, type JsonTreeLabels } from '../../presentation/primitives/JsonTree';
import { IconCheckOutline16, IconChevronRightOutline14, IconCopyOutline16, IconWrapLinesOutline16 } from '../../presentation/primitives/icons';
import { MarkdownText } from '../../presentation/markdown/MarkdownText';
import { CodeBlock } from '../../presentation/markdown/CodeBlock';
import { Artifact } from '../components/Artifact';
import { cellKind, cellLabel, isErrorRecord } from './TrajectoryCell';
import type { InspectableDisplayItem, TrajectoryFacet } from './layout';
import { formatDurationMillis, formatDurationMs, formatStartedAt, instantMillis, wireCount } from './timeline';
import css from './Trajectory.module.css';

type RecordState = 'complete' | 'running' | 'error';

function jsonLabels(tx: Translate): JsonTreeLabels {
  return {
    copyValue: tx('trajectory:copy.copy-value'),
    copyJson: tx('trajectory:copy.copy-json'),
    copyPath: tx('trajectory:copy.copy-path'),
    copyPrettyJson: tx('trajectory:copy.copy-pretty-json'),
    copyCompactJson: tx('trajectory:copy.copy-compact-json'),
    copied: tx('trajectory:copied'),
    copyFailed: tx('trajectory:copy.copy-failed'),
    collapseNode: tx('trajectory:collapse'),
    expandNode: tx('trajectory:expand'),
    copyButtonTitle: action => action,
  };
}

const TAB_LABEL = {
  overview: 'trajectory:tab.summary',
  rendered: 'trajectory:tab.preview',
  raw: 'trajectory:tab.raw',
  source: 'trajectory:tab.source',
  input: 'trajectory:tab.payload',
  output: 'trajectory:tab.result',
  schema: 'trajectory:tab.schema',
  timing: 'trajectory:tab.timing',
  options: 'trajectory:tab.options',
  usage: 'trajectory:tab.usage',
  'system-prompt': 'trajectory:tab.system-prompt',
  tools: 'trajectory:tab.tools',
  diff: 'trajectory:tab.diff',
} as const;

/** The Harness tab set for one selected item. */
export function inspectorTabs(item: InspectableDisplayItem): TrajectoryFacet[] {
  if (item.type === 'SystemPromptCell') return item.record.request?.system_prompt.state === 'changed' ? ['diff', 'system-prompt', 'tools'] : ['system-prompt', 'tools'];
  if (item.type === 'RequestBoundary') return ['overview', 'options', 'usage', 'timing'];
  if (item.type === 'ContextRow') return ['overview', 'rendered', 'raw', 'source'];
  switch (item.record.kind) {
    case 'user': return ['overview', 'rendered', 'raw', 'source'];
    case 'assistant': return ['overview', 'rendered', 'raw'];
    case 'compaction': return ['overview', 'raw'];
    case 'tool': return ['overview', 'input', 'output', 'schema', 'timing'];
    default: return ['overview', 'rendered', 'timing'];
  }
}

function stateOf(record: TraceRecord): RecordState {
  if (isErrorRecord(record)) return 'error';
  return record.state === 'completed' ? 'complete' : record.state === 'running' || record.state === 'pending' || record.state === 'waiting' ? 'running' : 'complete';
}

function StatusValue({ state }: { state: RecordState }) {
  const tx = useTranslation();
  return <dd className={state === 'error' ? css.error : undefined}>
    {tx(state === 'error' ? 'trajectory:status.failed' : state === 'running' ? 'trajectory:status.pending' : 'trajectory:status.completed')}
  </dd>;
}

/** Whether a click lands on an active text selection and should keep it. */
function clickSelectsText(target: Node): boolean {
  const selection = window.getSelection();
  return selection !== null && !selection.isCollapsed && selection.rangeCount > 0 && selection.getRangeAt(0).intersectsNode(target);
}

function StartedAtValue({ timestamp }: { timestamp: number | undefined }) {
  const tx = useTranslation();
  const [showUnix, setShowUnix] = useState(false);
  if (timestamp === undefined) return <dd>{tx('trajectory:timing.not-available')}</dd>;
  return <dd>
    <button type="button" className={css.timestampToggle} title={showUnix ? tx('trajectory:timing.show-local-time') : tx('trajectory:timing.show-unix-timestamp')}
      onClick={event => { if (!clickSelectsText(event.currentTarget)) setShowUnix(current => !current); }}>
      {showUnix ? (timestamp / 1_000).toFixed(3) : formatStartedAt(timestamp)}
    </button>
  </dd>;
}

function NoPayload({ children }: { children: ReactNode }) {
  return <p className={css.noPayload}>{children}</p>;
}

/** Detail-backed content waits for its read, or names a failed one. */
function DetailPending({ error }: { error: string | undefined }) {
  const tx = useTranslation();
  return error
    ? <p role="alert" className={`${css.noPayload} ${css.error}`}>{error}</p>
    : <p role="status" className={css.noPayload}>{tx('trajectory:details.loading')}</p>;
}

function Json({ value, label, preview = false, error = false }: { value: unknown; label: string; preview?: boolean; error?: boolean }) {
  const tx = useTranslation();
  const className = [preview ? css.jsonPreview : css.jsonPayload, error ? css.errorPayload : undefined].filter(Boolean).join(' ');
  if (value === null || typeof value !== 'object') return <pre className={[css.payload, preview ? css.payloadPreview : undefined, error ? css.errorPayload : undefined].filter(Boolean).join(' ')}>{JSON.stringify(value)}</pre>;
  return <JsonTree data={value} label={label} labels={jsonLabels(tx)} collapsedStringLines={preview ? 3 : 12} className={className} />;
}

function parseJsonContainer(value: string): object | undefined {
  try {
    const parsed: unknown = JSON.parse(value);
    return typeof parsed === 'object' && parsed !== null ? parsed : undefined;
  } catch {
    return undefined;
  }
}

/** A Harness overview section: a titled preview that opens its full tab. */
function OverviewSection({ label, onOpen, actions, children }: { label: string; onOpen: () => void; actions?: ReactNode; children: ReactNode }) {
  const preview = useRef<HTMLDivElement>(null);
  const [hasMore, setHasMore] = useState(false);
  const measure = useCallback((element: HTMLElement) => {
    setHasMore(element.scrollHeight - element.clientHeight - element.scrollTop > 1);
  }, []);
  useLayoutEffect(() => {
    const element = preview.current!;
    measure(element);
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(() => measure(element));
    observer.observe(element);
    for (const child of element.children) observer.observe(child);
    return () => observer.disconnect();
  }, [children, measure]);
  return <section className={css.overviewSection}>
    <h3 className={css.overviewHeading}>
      <button type="button" className={css.overviewTitle} onClick={onOpen}>
        <span>{label}</span>
        <IconChevronRightOutline14 className={css.overviewTitleIcon} size={12} />
      </button>
      {actions}
    </h3>
    <div ref={preview} className={`${css.overviewPreview} ${css.summaryScrollRegion}`} data-summary-scroll-region="" data-scroll-more={hasMore || undefined}
      onScroll={event => measure(event.currentTarget)}>
      {children}
    </div>
  </section>;
}

function NavLink({ label, onClick }: { label: string; onClick: () => void }) {
  return <button type="button" className={css.overviewHierarchyNavLink} onClick={onClick}>
    <span>{label}</span>
    <IconChevronRightOutline14 className={css.overviewHierarchyJumpIconTight} size={11} />
  </button>;
}

/** Disjoint token buckets, as Harness reports a request. */
interface Usage { input?: number; cacheRead?: number; output?: number; reasoning?: number }

/** Native input tokens include the cached share; Harness shows both and the rest. */
function usageOf(record: TraceRecord | undefined): Usage | undefined {
  const usage = record?.request?.usage;
  if (!usage) return undefined;
  const cached = usage.details?.cached_input_tokens ?? undefined;
  const reasoning = usage.details?.reasoning_tokens ?? undefined;
  return {
    input: usage.input_tokens - (cached ?? 0),
    ...(cached === undefined ? {} : { cacheRead: cached }),
    output: usage.output_tokens,
    ...(reasoning === undefined ? {} : { reasoning }),
  };
}

function UsageRows({ usage }: { usage: Usage | undefined }) {
  const tx = useTranslation();
  if (usage === undefined) return <NoPayload>{tx('trajectory:usage.not-reported')}</NoPayload>;
  const tokens = (value: number) => tx('trajectory:unit.tokens', { value });
  const totalInput = usage.input === undefined && usage.cacheRead === undefined ? undefined : (usage.input ?? 0) + (usage.cacheRead ?? 0);
  return <dl className={css.overview}>
    {totalInput !== undefined && <div><dt>{tx('trajectory:usage.input')}</dt><dd>{tokens(totalInput)}</dd></div>}
    {usage.cacheRead !== undefined && <div className={css.requestTokenDetail}><dt>{tx('trajectory:usage.cached')}</dt><dd>{tokens(usage.cacheRead)}</dd></div>}
    {usage.input !== undefined && <div className={css.requestTokenDetail}><dt>{tx('trajectory:usage.other')}</dt><dd>{tokens(usage.input)}</dd></div>}
    {usage.output !== undefined && <div><dt>{tx('trajectory:usage.output')}</dt><dd>{tokens(usage.output)}</dd></div>}
    {usage.reasoning !== undefined && <div className={css.requestTokenDetail}><dt>{tx('trajectory:usage.reasoning')}</dt><dd>{tokens(usage.reasoning)}</dd></div>}
    {usage.output !== undefined && usage.reasoning !== undefined && <div className={css.requestTokenDetail}><dt>{tx('trajectory:usage.content')}</dt><dd>{tokens(usage.output - usage.reasoning)}</dd></div>}
  </dl>;
}

function TokenRows({ usage }: { usage: Usage | undefined }) {
  const tx = useTranslation();
  const tokens = (value: number) => tx('trajectory:unit.tokens', { value });
  return <>
    <div><dt>{tx('trajectory:usage.tokens')}</dt><dd>{usage?.output === undefined ? '—' : tokens(usage.output)}</dd></div>
    {usage?.reasoning !== undefined && <div className={css.requestTokenDetail}><dt>{tx('trajectory:usage.reasoning')}</dt><dd>{tokens(usage.reasoning)}</dd></div>}
    {usage?.output !== undefined && usage.reasoning !== undefined && <div className={css.requestTokenDetail}><dt>{tx('trajectory:usage.content')}</dt><dd>{tokens(Math.max(0, usage.output - usage.reasoning))}</dd></div>}
  </>;
}

/** Harness's assistant timing panel, from the request's native generation evidence. */
function RequestTiming({ request }: { request: TraceRecord }) {
  const tx = useTranslation();
  const generation = request.request?.generation;
  const total = wireCount(request.timing.duration_ms);
  const ttft = wireCount(generation?.ttft_ms);
  const decoding = wireCount(generation?.generation_ms);
  const rate = generation?.output_tokens_per_second ?? undefined;
  const running = stateOf(request) === 'running';
  return <dl className={css.overview}>
    <div><dt>{tx('trajectory:timing.started')}</dt><StartedAtValue timestamp={instantMillis(request.timing.started_at)} /></div>
    <div><dt>{tx('trajectory:timing.total-duration')}</dt><dd>{total !== undefined ? formatDurationMs(tx, total) : running ? tx('trajectory:status.pending') : tx('trajectory:timing.not-recorded')}</dd></div>
    <div><dt>{tx('trajectory:timing.ttft')}</dt><dd>{ttft !== undefined ? formatDurationMs(tx, ttft) : tx('trajectory:timing.first-token-unavailable')}</dd></div>
    <div><dt>{tx('trajectory:timing.generation')}</dt><dd>{decoding !== undefined ? formatDurationMs(tx, decoding) : running ? tx('trajectory:status.pending') : tx('trajectory:timing.first-token-unavailable')}</dd></div>
    <div><dt>{tx('trajectory:timing.throughput')}</dt><dd>{rate !== undefined ? tx('trajectory:unit.tokens-per-second', { value: rate.toFixed(1) })
      : !request.request?.usage ? tx('trajectory:timing.usage-unavailable') : running ? tx('trajectory:status.pending') : tx('trajectory:timing.first-token-unavailable')}</dd></div>
  </dl>;
}

function RecordTiming({ record, preview = false }: { record: TraceRecord; preview?: boolean }) {
  const tx = useTranslation();
  const duration = wireCount(record.timing.duration_ms);
  return <dl className={css.overview}>
    <div><dt>{tx('trajectory:timing.started')}</dt><StartedAtValue timestamp={instantMillis(record.timing.started_at)} /></div>
    <div><dt>{tx('trajectory:timing.duration')}</dt><dd>{formatDurationMillis(tx, duration)}</dd></div>
    {!preview && <div><dt>{tx('trajectory:timing.source')}</dt><dd>{duration === undefined ? tx('trajectory:timing.not-available') : tx('trajectory:timing.session-timestamps')}</dd></div>}
  </dl>;
}

/** The text a message's blocks contribute to Markdown, in native order. */
function blockText(blocks: readonly TraceContentBlock[], type: 'text' | 'reasoning'): string {
  return blocks.flatMap(block => block.type === type || (type === 'text' && block.type === 'refusal') ? [block.text.text] : []).filter(Boolean).join('\n\n');
}

function blockArtifacts(blocks: readonly TraceContentBlock[]): TraceArtifact[] {
  return blocks.flatMap(block => block.type === 'image' ? [{ ...block.artifact, name: block.alt ?? block.artifact.name }] : block.type === 'file' ? [block.artifact] : []);
}

function rawBlockContent(block: TraceContentBlock): string {
  switch (block.type) {
    case 'text': case 'reasoning': case 'refusal': return block.text.text;
    case 'json': return JSON.stringify(block.value.value, null, 2);
    case 'tool_call': return JSON.stringify(block.arguments.value);
    case 'tool_result': return block.blocks.map(rawBlockContent).join('\n');
    case 'image': return block.alt ?? block.artifact.name ?? block.artifact.artifact_id;
    case 'file': return block.artifact.name ?? block.artifact.artifact_id;
    case 'upload': return block.name;
  }
}

function MarkdownFragment({ text, preview, compact = false }: { text: string; preview: boolean; compact?: boolean }) {
  return <div className={preview ? css.markdownPreview : css.markdownPayload}>
    <MarkdownText text={text} variant={compact ? 'compact' : 'normal'} />
  </div>;
}

function Attachments({ artifacts, preview }: { artifacts: readonly TraceArtifact[]; preview: boolean }) {
  const tx = useTranslation();
  if (!artifacts.length) return null;
  return <ul className={preview ? `${css.attachments} ${css.attachmentsPreview}` : css.attachments} aria-label={tx('trajectory:attachment.list')}>
    {artifacts.map(artifact => <li key={artifact.artifact_id} className={css.attachmentRow}>
      <Artifact id={artifact.artifact_id} name={artifact.name ?? artifact.artifact_id} image={artifact.image} mimeType={artifact.mime_type ?? undefined} />
    </li>)}
  </ul>;
}

function ToolCalls({ blocks, preview, onOpenCall }: { blocks: readonly TraceContentBlock[]; preview: boolean; onOpenCall: (callId: string) => void }) {
  const tx = useTranslation();
  const calls = blocks.flatMap(block => block.type === 'tool_call' ? [block] : []);
  if (!calls.length) return null;
  return <ul className={preview ? `${css.assistantToolCalls} ${css.assistantToolCallsPreview}` : css.assistantToolCalls}>
    {calls.map(call => <li key={call.call_id}>
      <button type="button" className={css.assistantToolCallButton} title={tx('trajectory:block.open-summary-title')} onClick={() => onOpenCall(call.call_id)}>
        <svg className={css.assistantToolCallIcon} width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94z" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
        <span className={css.assistantToolCallText}>
          <span className={css.assistantToolCallName}>{call.name}</span>
          <span className={css.assistantToolCallArgs}>{JSON.stringify(call.arguments.value)}</span>
        </span>
      </button>
    </li>)}
  </ul>;
}

function SourceBlocks({ blocks, onOpenCall }: { blocks: readonly TraceContentBlock[]; onOpenCall: (callId: string) => void }) {
  const tx = useTranslation();
  return <div className={css.sourceBlocks}>
    {blocks.map((block, index) => {
      const label = <span className={css.sourceBlockLabel}>{tx('trajectory:block.label', { index: index + 1, type: block.type })}</span>;
      return <section className={css.sourceBlock} key={index}>
        {block.type === 'tool_call'
          ? <button type="button" className={css.sourceBlockJumpTarget} aria-label={tx('trajectory:block.open-summary', { index: index + 1 })} title={tx('trajectory:block.open-summary-title')} onClick={() => onOpenCall(block.call_id)}>
            {label}<IconChevronRightOutline14 className={css.sourceBlockJumpIcon} size={12} />
          </button>
          : <div className={css.sourceBlockHeader}>{label}</div>}
        <pre className={css.sourceBlockContent}>{rawBlockContent(block)}</pre>
      </section>;
    })}
  </div>;
}

/** Harness Markdown record content: thinking quote, output, calls and attachments. */
function MessageContent({ blocks, fallback, artifacts, rendered, preview = false, thinkingExpanded, onThinkingExpanded, onOpenCall }: {
  blocks: readonly TraceContentBlock[] | undefined;
  fallback: string | undefined;
  artifacts: readonly TraceArtifact[];
  rendered: boolean;
  preview?: boolean;
  thinkingExpanded: boolean;
  onThinkingExpanded: (expanded: boolean) => void;
  onOpenCall: (callId: string) => void;
}) {
  const tx = useTranslation();
  if (!rendered && blocks?.length) return <SourceBlocks blocks={blocks} onOpenCall={onOpenCall} />;
  const output = blocks ? blockText(blocks, 'text') : fallback ?? '';
  const thinking = blocks ? blockText(blocks, 'reasoning') : '';
  const files = [...new Map([...artifacts, ...blockArtifacts(blocks ?? [])].map(artifact => [artifact.artifact_id, artifact])).values()];
  const calls = blocks?.some(block => block.type === 'tool_call') === true;
  if (!rendered) return <pre className={`${css.payload} ${preview ? css.payloadPreview : ''}`}>{output}</pre>;
  if (!output && !thinking && !files.length && !calls) return <NoPayload>{tx('trajectory:record.no-content')}</NoPayload>;
  return <div className={thinking ? `${css.assistantContent} ${css.assistantContentRendered}` : undefined}>
    {thinking && <div className={preview && !output ? `${css.thinkingQuote} ${css.thinkingQuoteOnlyPreview}` : css.thinkingQuote}>
      <button type="button" className={css.thinkingToggle} aria-expanded={thinkingExpanded} onClick={() => onThinkingExpanded(!thinkingExpanded)}>
        {tx('trajectory:record.thinking')}
        <IconChevronRightOutline14 className={css.thinkingChevron} size={12} />
      </button>
      {thinkingExpanded && <MarkdownFragment text={thinking} preview={preview} compact />}
    </div>}
    {output && <div className={css.assistantOutput}><MarkdownFragment text={output} preview={preview} /></div>}
    <ToolCalls blocks={blocks ?? []} preview={preview} onOpenCall={onOpenCall} />
    <Attachments artifacts={files} preview={preview} />
  </div>;
}

/** A Tool result's blocks, as Harness shows its output blocks. */
function ToolResult({ record, detail, preview = false }: { record: TraceRecord; detail: TraceDetail['tool']; preview?: boolean }) {
  const tx = useTranslation();
  const result = detail?.result;
  if (!result) return <NoPayload>{tx(stateOf(record) === 'running' ? 'trajectory:code.running' : 'trajectory:record.no-result')}</NoPayload>;
  const error = result.outcome !== 'success';
  const text = result.blocks.length === 1 && result.blocks[0]!.type === 'text' ? result.blocks[0]!.text.text : undefined;
  const json = text === undefined ? undefined : parseJsonContainer(text);
  const artifacts = [...new Map([...record.attachments, ...result.attachments].map(artifact => [artifact.artifact_id, artifact])).values()];
  if (json !== undefined && !error) return <Json value={json} label={tx('trajectory:record.result-json')} preview={preview} />;
  return <div className={[css.resultBlocks, preview ? css.resultBlocksPreview : undefined, error ? css.errorPayload : undefined].filter(Boolean).join(' ')}>
    {error && result.detail?.text && <pre className={css.resultBlockText}>{result.detail.text}</pre>}
    {result.blocks.map((block, index) => block.type === 'json'
      ? <Json key={index} value={block.value.value} label={tx('trajectory:record.result-json')} preview={preview} error={error} />
      : block.type === 'image' || block.type === 'file' ? null
        : rawBlockContent(block) !== '' ? <pre key={index} className={css.resultBlockText}>{rawBlockContent(block)}</pre> : null)}
    {!result.blocks.length && !(error && result.detail?.text) && <pre className={`${css.resultBlockText} ${css.noOutputText}`}>{tx('trajectory:record.no-output')}</pre>}
    <Attachments artifacts={artifacts} preview={preview} />
  </div>;
}

function ToolPayload({ value, preview = false }: { value: TraceJson | null | undefined; preview?: boolean }) {
  const tx = useTranslation();
  if (!value) return <NoPayload>{tx('trajectory:record.no-payload')}</NoPayload>;
  return <Json value={value.value} label={tx('trajectory:record.payload-json')} preview={preview} />;
}

function Schema({ definition, preview = false }: { definition: TraceToolDefinition | null | undefined; preview?: boolean }) {
  const tx = useTranslation();
  if (!definition) return <NoPayload>{tx('trajectory:record.schema-unavailable')}</NoPayload>;
  const parameters = definition.input_schema.value;
  return <div className={preview ? `${css.schema} ${css.schemaPreview}` : css.schema}>
    <header className={css.schemaIntro}>
      <h3 className={css.schemaName}>{definition.name}</h3>
      <p className={css.schemaDescription}>{definition.description.text}</p>
    </header>
    <section className={css.schemaParameters}>
      <h4 className={css.schemaParametersTitle}>{tx('trajectory:record.parameters')}</h4>
      {parameters !== null && typeof parameters === 'object'
        ? <JsonTree data={parameters} label={tx('trajectory:record.named-parameters-json', { name: definition.name })} labels={jsonLabels(tx)} collapsedStringLines={preview ? 3 : 12} className={css.schemaTree} />
        : <pre className={`${css.payload} ${preview ? css.payloadPreview : ''}`}>{JSON.stringify(parameters)}</pre>}
    </section>
  </div>;
}

function CopyAction({ text, label }: { text: string; label: string }) {
  const tx = useTranslation();
  const [state, setState] = useState<'idle' | 'copied' | 'failed'>('idle');
  useEffect(() => {
    if (state === 'idle') return;
    const timer = setTimeout(() => setState('idle'), 1_500);
    return () => clearTimeout(timer);
  }, [state]);
  const title = state === 'idle' ? label : tx(state === 'copied' ? 'trajectory:copied' : 'trajectory:copy.copy-failed');
  return <button type="button" className={css.programAction} data-state={state} aria-label={title} title={title}
    onClick={() => { void writeClipboard(text).then(ok => setState(ok ? 'copied' : 'failed')); }}>
    {state === 'copied' ? <IconCheckOutline16 size={12} /> : <IconCopyOutline16 size={12} />}
  </button>;
}

/** Harness program input: native program source with wrap, raw-JSON and copy actions. */
function ProgramInput({ source, language, args, onOpen }: { source: string; language: string | undefined; args: TraceJson | null | undefined; onOpen?: () => void }) {
  const tx = useTranslation();
  const contents = useId();
  const [wrapped, setWrapped] = useState(false);
  const [showJson, setShowJson] = useState(false);
  const actions = <span className={css.programActions}>
    {!showJson && language !== undefined && <span className={css.programLanguage}>{language}</span>}
    {!showJson && <button type="button" className={css.programAction} aria-label={tx('trajectory:record.wrap-lines')} title={tx('trajectory:record.wrap-lines')} aria-pressed={wrapped} aria-controls={contents} onClick={() => setWrapped(!wrapped)}>
      <IconWrapLinesOutline16 size={12} />
    </button>}
    {onOpen === undefined && args && <button type="button" className={css.programAction} aria-label={tx('trajectory:code.original-json')} title={tx('trajectory:code.original-json')} aria-pressed={showJson} aria-controls={contents} onClick={() => setShowJson(value => !value)}>
      <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <path d="M6 2H5a2 2 0 0 0-2 2v2a2 2 0 0 1-2 2 2 2 0 0 1 2 2v2a2 2 0 0 0 2 2h1" /><path d="M10 2h1a2 2 0 0 1 2 2v2a2 2 0 0 0 2 2 2 2 0 0 0-2 2v2a2 2 0 0 1-2 2h-1" />
      </svg>
    </button>}
    <CopyAction text={showJson && args ? JSON.stringify(args.value) : source} label={tx(showJson ? 'trajectory:copy.copy-json' : 'trajectory:code.copy-source')} />
  </span>;
  const body = <div id={contents} className={css.programContent} data-wrap={wrapped}>
    {showJson && args
      ? <Json value={args.value} label={tx('trajectory:record.parameters-json')} />
      : <CodeBlock code={source} lang={language} lineNumbers className={css.programSource} copyLabel={tx('trajectory:code.copy-source')} copiedLabel={tx('trajectory:copied')} />}
  </div>;
  return onOpen === undefined
    ? <section className={css.programPanel}><header className={css.overviewHeading}><span>{tx('trajectory:code.source')}</span>{actions}</header>{body}</section>
    : <OverviewSection label={tx('trajectory:code.source')} onOpen={onOpen} actions={actions}>{body}</OverviewSection>;
}

function promptDiffLines(before: string, after: string) {
  const patch = structuredPatch('', '', before, after, undefined, undefined, { context: 3 });
  return patch.hunks.flatMap((hunk, index) => [
    ...(index === 0 ? [] : [{ kind: 'meta' as const, text: '' }]),
    { kind: 'meta' as const, text: `@@ -${hunk.oldStart},${hunk.oldLines} +${hunk.newStart},${hunk.newLines} @@` },
    ...hunk.lines.flatMap(line => line.startsWith('\\') ? []
      : [{ kind: line.startsWith('+') ? 'added' as const : line.startsWith('-') ? 'removed' as const : 'context' as const, text: line }]),
  ]);
}

const DIFF_LINE = { meta: css.promptDiffLinemeta, context: css.promptDiffLinecontext, added: css.promptDiffLineadded, removed: css.promptDiffLineremoved };

/** A diff needs both complete frozen prompts; anything less is named, never diffed. */
function SystemPromptDiff({ detail }: { detail: NonNullable<TraceDetail['request']> }) {
  const tx = useTranslation();
  const previous = detail.previous_system_prompt;
  const current = detail.effective_system_prompt;
  if (detail.predecessor.availability === 'not_applicable') return <NoPayload>{tx('trajectory:trajectory-inspector.no-predecessor-initial-prompt')}</NoPayload>;
  if (!previous) return <NoPayload>{tx('trajectory:trajectory-inspector.previous-prompt-unavailable-a-complete-diff-cannot-be-produced')}</NoPayload>;
  if (previous.truncated || current.truncated) {
    return <NoPayload>{tx('trajectory:trajectory-inspector.a-complete-diff-cannot-be-produced')} {[
      current.truncated ? tx('trajectory:trajectory-inspector.current-prompt-truncated') : undefined,
      previous.truncated ? tx('trajectory:trajectory-inspector.previous-prompt-truncated') : undefined,
    ].filter(Boolean).join('; ')}</NoPayload>;
  }
  const lines = promptDiffLines(previous.text, current.text);
  return <div className={css.promptDiffSections}>
    {lines.length > 0 && <section className={css.promptDiffSection}>
      <h3 className={css.promptDiffTitle}>{tx('trajectory:record.system-prompt')}</h3>
      <pre className={css.promptDiff}>{lines.map((line, index) => <span className={DIFF_LINE[line.kind]} key={index}>{line.text || ' '}{'\n'}</span>)}</pre>
    </section>}
  </div>;
}

function ToolCatalog({ tools }: { tools: readonly TraceToolDefinition[] }) {
  const tx = useTranslation();
  if (!tools.length) return <NoPayload>{tx('trajectory:record.tools-missing')}</NoPayload>;
  return <div className={css.toolCatalog}>
    {tools.map((tool, index) => <details className={css.toolCatalogItem} key={`${tool.tool_id}:${index}`}>
      <summary className={css.toolCatalogSummary}>
        <IconChevronRightOutline14 className={css.toolCatalogChevron} size={12} />
        <svg className={css.toolCatalogIcon} width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94z" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
        <span className={css.toolCatalogName}>{tool.name}</span>
        <span className={css.toolCatalogDescription}>{tool.description.text}</span>
      </summary>
      <div className={css.toolCatalogDefinition}>
        {tool.description.text !== '' && <p className={css.toolCatalogFullDescription}>{tool.description.text}</p>}
        {tool.input_schema.value !== null && typeof tool.input_schema.value === 'object'
          ? <JsonTree data={tool.input_schema.value} label={tx('trajectory:record.named-parameters-json', { name: tool.name })} labels={jsonLabels(tx)} className={css.toolCatalogTree} />
          : <pre className={css.payload}>{JSON.stringify(tool.input_schema.value)}</pre>}
      </div>
    </details>)}
  </div>;
}

/** The native producer of a User or Context message, in Harness's source wording. */
function sourceLabel(tx: Translate, source: unknown): string {
  if (typeof source === 'string') return source === 'human' ? tx('trajectory:source.user') : `${source[0]?.toUpperCase() ?? ''}${source.slice(1)}`;
  if (source && typeof source === 'object' && 'type' in source) {
    const value = source as { type: string; contributor?: string };
    if (value.type === 'certified_extension' && value.contributor) return tx('trajectory:source.plugin-named', { plugin: value.contributor });
    return `${value.type[0]?.toUpperCase() ?? ''}${value.type.slice(1)}`;
  }
  return tx('trajectory:source.unknown');
}

function MessageSource({ source }: { source: unknown }) {
  const tx = useTranslation();
  if (source === undefined) return <NoPayload>{tx('trajectory:source.not-recorded')}</NoPayload>;
  return <Json value={typeof source === 'object' && source !== null ? source : { value: source }} label={tx('trajectory:source.message-json')} />;
}

/** Props for the Trajectory record inspector. */
export interface TrajectoryInspectorProps {
  item: InspectableDisplayItem;
  facet: TrajectoryFacet;
  onFacet: (facet: TrajectoryFacet) => void;
  detail?: TraceDetail | undefined;
  loading?: boolean | undefined;
  error?: string | undefined;
  /** Request the heavy detail of this record; the owner fences the reply. */
  onLoadDetail: (id: string) => void;
  onClose: () => void;
  /** Turn and group labels of the selected record in the loaded ledger. */
  location?: { turn?: string | undefined; group?: string | undefined } | undefined;
  /** The loaded window, for the record's own request, message and calls. */
  records: readonly TraceRecord[];
  /** Assistant record id → its loaded matching Tool executions. */
  executions: ReadonlyMap<string, readonly TraceRecord[]>;
  requestNumbers: ReadonlyMap<string, number>;
  /** Whether the loaded window starts at the conversation's first record. */
  completeHistory: boolean;
  /** Open another loaded record's summary (or a named tab). */
  onOpen: (recordId: string, facet?: TrajectoryFacet) => void;
}

/**
 * Render the inspector for one selected item.
 * @param props - the selected item, its fetched detail and navigation.
 * @returns the details panel contents.
 */
export function TrajectoryInspector({ item, facet, onFacet, detail, loading, error, onLoadDetail, onClose, location, records, executions, requestNumbers, completeHistory, onOpen }: TrajectoryInspectorProps) {
  const tx = useTranslation();
  const record = item.record;
  useEffect(() => {
    if (record.has_detail && !detail && !loading && !error) onLoadDetail(record.id);
  }, [record.id, record.has_detail, detail, loading, error, onLoadDetail]);
  const tabs = inspectorTabs(item);
  const active = tabs.includes(facet) ? facet : tabs[0]!;
  // An Assistant record's own request is the one that accepted its message.
  const ownRequest = record.kind === 'assistant' ? records.find(candidate => candidate.request?.assistant_message_id != null && candidate.request.assistant_message_id === record.message_id) : undefined;
  const assistantOf = (id: string) => [...executions].find(([, tools]) => tools.some(tool => tool.id === id))?.[0];
  const parentAssistant = record.kind === 'tool' ? assistantOf(record.id) : undefined;
  const requestAssistant = item.type === 'RequestBoundary' ? records.find(candidate => candidate.kind === 'assistant' && candidate.message_id != null && candidate.message_id === record.request?.assistant_message_id) : undefined;
  const openCall = (callId: string) => {
    const tool = records.find(candidate => candidate.kind === 'tool' && candidate.tool?.call_id === callId && candidate.location.attempt_id === record.location.attempt_id);
    if (tool) onOpen(tool.id);
  };
  const [thinkingExpanded, setThinkingExpanded] = useState(true);
  const messages = item.type === 'ContextRow'
    ? detail?.request?.messages.filter(message => message.message_id === item.context_message_id)
    : detail?.messages;
  // A detail with no canonical message leaves the record's own preview as the content.
  const blocks = messages?.length ? messages.flatMap(message => message.blocks) : undefined;
  const fallback = item.type === 'ContextRow' ? item.context.preview?.text : record.preview?.text;
  const artifacts = item.type === 'ContextRow' ? item.context.attachments : record.attachments;
  const source = item.type === 'ContextRow' ? item.context.source : messages?.[0]?.source ?? undefined;
  const kind = cellKind(item);
  // Admitted input cells are complete facts, whatever their Request later did.
  const state = item.type === 'ContextRow' || item.type === 'SystemPromptCell' ? 'complete' : stateOf(record);
  const toolDetail = detail?.tool ?? undefined;
  const program = toolDetail?.source ?? undefined;
  const usage = usageOf(item.type === 'RequestBoundary' ? record : ownRequest);
  const cumulative = (() => {
    if (!completeHistory) return undefined;
    const index = records.indexOf(record);
    return records.slice(0, index + 1).reduce<Usage | undefined>((sum, candidate) => {
      const next = usageOf(candidate);
      if (!next) return sum;
      return { input: (sum?.input ?? 0) + (next.input ?? 0), cacheRead: (sum?.cacheRead ?? 0) + (next.cacheRead ?? 0), output: (sum?.output ?? 0) + (next.output ?? 0), reasoning: (sum?.reasoning ?? 0) + (next.reasoning ?? 0) };
    }, undefined);
  })();
  const requestDetail = detail?.request ?? undefined;
  const options = requestDetail ? {
    model: requestDetail.model,
    protocol: requestDetail.protocol,
    thinking: requestDetail.reasoning_enabled ? 'enabled' : 'disabled',
    ...(requestDetail.reasoning_profile ? { reasoningEffort: requestDetail.reasoning_profile } : {}),
    maxTokens: requestDetail.max_output_tokens,
    contextWindowTokens: requestDetail.context_window_tokens,
    ...Object.fromEntries(requestDetail.options.map(option => [option.name, option.value.value])),
  } : undefined;
  const pending = record.has_detail && !detail;
  const requestNumber = requestNumbers.get(record.id);
  const content = (rendered: boolean, preview = false) => pending && !fallback
    ? <DetailPending error={error} />
    : <MessageContent blocks={blocks} fallback={fallback} artifacts={artifacts} rendered={rendered} preview={preview} thinkingExpanded={thinkingExpanded} onThinkingExpanded={setThinkingExpanded} onOpenCall={openCall} />;
  const tabLabel = (tab: TrajectoryFacet) => tab === 'input' && program ? tx('trajectory:code.source') : tab === 'raw' && record.kind === 'compaction' ? tx('trajectory:tab.raw-output') : tx(TAB_LABEL[tab]);
  const title = item.type === 'RequestBoundary'
    ? <>
      <span className={css.requestDetailsDot} aria-hidden="true" />
      <span className={css.requestDetailsName}>{tx('trajectory:request.label', { request: requestNumber ?? '—' })}</span>
      <span className={css.detailsLocation}>{location?.turn ?? tx('trajectory:section.between-turns')}</span>
    </>
    : <>
      <span className={css.kindTag} data-role-kind={kind}>{cellLabel(tx, kind)}</span>
      <span className={css.detailsLocation}>{item.type === 'SystemPromptCell' ? item.label
        : [location?.turn ?? tx('trajectory:section.between-turns'), record.kind === 'compaction' ? undefined : location?.group].filter(Boolean).join(' · ')}</span>
    </>;
  const panelClass = active === 'overview' ? `${css.detailBody} ${css.detailBodySummary}` : program && active === 'input' ? `${css.detailBody} ${css.detailBodyProgram}` : css.detailBody;

  const overview = () => {
    if (item.type === 'RequestBoundary') {
      const failure = requestDetail?.failure?.message.text ?? record.request?.failure_kind ?? undefined;
      return <>
        <dl className={`${css.overview} ${css.summaryScrollRegion}`} data-summary-scroll-region="">
          <div><dt>{tx('trajectory:details.status')}</dt><StatusValue state={state} /></div>
          {record.request && <div><dt>{tx('trajectory:details.model')}</dt><dd>{record.request.model}</dd></div>}
          <div><dt>{tx('trajectory:details.tool-calls')}</dt><dd>{records.filter(candidate => candidate.kind === 'tool' && candidate.location.attempt_id === record.location.attempt_id && candidate.location.step_id === record.location.step_id).length}</dd></div>
          {failure && <div><dt>{tx('trajectory:details.error')}</dt><dd className={css.error}>{failure}</dd></div>}
          {requestAssistant && <div><dt>{tx('trajectory:details.result')}</dt><dd className={css.overviewParentLinks}><NavLink label={tx('trajectory:details.assistant-message')} onClick={() => onOpen(requestAssistant.id)} /></dd></div>}
        </dl>
        <div className={css.overviewSections}>
          <OverviewSection label={tx('trajectory:tab.options')} onOpen={() => onFacet('options')}>
            {options ? <Json value={options} label={tx('trajectory:options.json')} preview /> : pending ? <DetailPending error={error} /> : <NoPayload>{tx('trajectory:options.not-recorded')}</NoPayload>}
          </OverviewSection>
          <OverviewSection label={tx('trajectory:tab.usage')} onOpen={() => onFacet('usage')}><UsageRows usage={usage} /></OverviewSection>
          <OverviewSection label={tx('trajectory:tab.timing')} onOpen={() => onFacet('timing')}><RequestTiming request={record} /></OverviewSection>
        </div>
      </>;
    }
    if (record.kind === 'compaction') {
      return <>
        <dl className={`${css.overview} ${css.summaryScrollRegion}`} data-summary-scroll-region="">
          <div><dt>{tx('trajectory:details.status')}</dt><StatusValue state={state} /></div>
          <div><dt>{tx('trajectory:timing.duration')}</dt><dd>{formatDurationMillis(tx, wireCount(record.timing.duration_ms))}</dd></div>
          <div><dt>{tx('trajectory:usage.tokens')}</dt><dd>—</dd></div>
        </dl>
        {(blocks?.length || fallback) && <div className={`${css.compactedSummary} ${css.summaryScrollRegion}`} data-summary-scroll-region="">{content(true)}</div>}
      </>;
    }
    const markdown = record.kind !== 'tool';
    return <>
      <dl className={`${css.overview} ${css.summaryScrollRegion}`} data-summary-scroll-region="">
        {(item.type === 'ContextRow' || record.kind === 'user') && source !== undefined && <div>
          <dt>{tx('trajectory:details.source')}</dt>
          <dd className={css.overviewParentLinks}><NavLink label={sourceLabel(tx, source)} onClick={() => onFacet('source')} /></dd>
        </div>}
        {ownRequest && <div>
          <dt>{tx('trajectory:details.source')}</dt>
          <dd className={css.overviewParentLinks}><NavLink label={tx('trajectory:request.label', { request: requestNumbers.get(ownRequest.id) ?? '—' })} onClick={() => onOpen(ownRequest.id)} /></dd>
        </div>}
        {parentAssistant && <div>
          <dt>{tx('trajectory:details.hierarchy')}</dt>
          <dd className={css.overviewParentLinks}><NavLink label={tx('trajectory:details.assistant-message')} onClick={() => onOpen(parentAssistant)} /></dd>
        </div>}
        <div><dt>{tx('trajectory:details.status')}</dt><StatusValue state={state} /></div>
        {record.kind === 'assistant' && <TokenRows usage={usage} />}
        {(item.type === 'ContextRow' || record.kind === 'user') && <div><dt>{tx('trajectory:timing.duration')}</dt><dd>{formatDurationMillis(tx, wireCount(record.timing.duration_ms) ?? 0)}</dd></div>}
      </dl>
      <div className={css.overviewSections}>
        {markdown
          ? <OverviewSection label={tx('trajectory:tab.preview')} onOpen={() => onFacet('rendered')}>{content(true, true)}</OverviewSection>
          : record.kind === 'tool'
            ? <>
              {program
                ? <ProgramInput source={program.text.text} language={program.language ?? undefined} args={toolDetail?.arguments} onOpen={() => onFacet('input')} />
                : <OverviewSection label={tx('trajectory:tab.payload')} onOpen={() => onFacet('input')}>{pending ? <DetailPending error={error} /> : <ToolPayload value={toolDetail?.arguments} preview />}</OverviewSection>}
              <OverviewSection label={tx('trajectory:tab.result')} onOpen={() => onFacet('output')}>{pending ? <DetailPending error={error} /> : <ToolResult record={record} detail={toolDetail} preview />}</OverviewSection>
              <OverviewSection label={tx('trajectory:tab.schema')} onOpen={() => onFacet('schema')}>{pending ? <DetailPending error={error} /> : <Schema definition={toolDetail?.definition} preview />}</OverviewSection>
            </>
            : null}
        {ownRequest && <OverviewSection label={tx('trajectory:timing.request')} onOpen={() => onOpen(ownRequest.id, 'timing')}><RequestTiming request={ownRequest} /></OverviewSection>}
        {record.kind !== 'user' && record.kind !== 'assistant' && item.type !== 'ContextRow' && <OverviewSection label={tx('trajectory:tab.timing')} onOpen={() => onFacet('timing')}><RecordTiming record={record} preview /></OverviewSection>}
      </div>
    </>;
  };

  const body = () => {
    switch (active) {
      case 'overview': return overview();
      case 'system-prompt': {
        if (!requestDetail) return <DetailPending error={error} />;
        const prompt = requestDetail.effective_system_prompt.text;
        return prompt === ''
          ? <NoPayload>{tx('trajectory:record.system-prompt-missing')}</NoPayload>
          : <div className={`${css.markdownPayload} ${css.systemPrompt}`}><MarkdownText text={prompt} /></div>;
      }
      case 'tools': return requestDetail ? <ToolCatalog tools={requestDetail.tools} /> : <DetailPending error={error} />;
      case 'diff': return requestDetail
        ? <SystemPromptDiff detail={requestDetail} />
        : <DetailPending error={error} />;
      case 'rendered': return content(true);
      case 'raw': return content(false);
      case 'source': return pending && item.type !== 'ContextRow' ? <DetailPending error={error} /> : <MessageSource source={source} />;
      case 'input':
        if (pending) return <DetailPending error={error} />;
        return program ? <ProgramInput source={program.text.text} language={program.language ?? undefined} args={toolDetail?.arguments} /> : <ToolPayload value={toolDetail?.arguments} />;
      case 'output': return pending ? <DetailPending error={error} /> : <ToolResult record={record} detail={toolDetail} />;
      case 'schema': return pending ? <DetailPending error={error} /> : <Schema definition={toolDetail?.definition} />;
      case 'options': return options ? <Json value={options} label={tx('trajectory:options.json')} /> : pending ? <DetailPending error={error} /> : <NoPayload>{tx('trajectory:options.not-recorded')}</NoPayload>;
      case 'usage': return <div className={css.usagePanel}>
        <section className={css.usageGroup}><h4 className={css.usageHeading}>{tx('trajectory:usage.this-request')}</h4><UsageRows usage={usage} /></section>
        <section className={css.usageGroup}><h4 className={css.usageHeading}>{tx('trajectory:usage.session-cumulative')}</h4><UsageRows usage={cumulative} /></section>
      </div>;
      case 'timing': return item.type === 'RequestBoundary' ? <RequestTiming request={record} /> : <RecordTiming record={record} />;
    }
  };

  return <>
    <div className={css.detailsHeader}>
      <div className={css.detailsTitle}>{title}</div>
      <button type="button" className={css.close} aria-label={tx('trajectory:details.close')} onClick={onClose}><span aria-hidden="true">×</span></button>
    </div>
    <div className={css.detailTabs} role="tablist" aria-label={tx('trajectory:details.event')}>
      {tabs.map(tab => <button key={tab} id={`trajectory-detail-${tab}`} type="button" role="tab" aria-controls="trajectory-detail-panel" aria-selected={active === tab}
        className={active === tab ? `${css.detailTab} ${css.detailTabActive}` : css.detailTab} onClick={() => onFacet(tab)}>
        {tabLabel(tab)}
      </button>)}
    </div>
    <div id="trajectory-detail-panel" className={panelClass} role="tabpanel" aria-labelledby={`trajectory-detail-${active}`}>
      {body()}
    </div>
  </>;
}
