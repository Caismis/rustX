import { message } from '../../locale/translation';
import type { Translate, TranslationKey } from '../../locale/translation';
import { useTranslation, useNotice } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness ui-trajectory/TrajectoryTable.tsx inspector; see PROVENANCE.md. */
/**
 * The record inspector.
 *
 * Sections vary by record kind, the way the Harness inspector does, and each
 * value is rendered with the primitive that matches its semantics: prose and
 * reasoning as Markdown, structured arguments and results through the JSON
 * tree, native program source through the code renderer, durable artifacts
 * through the existing artifact carrier. Nothing is dumped into a `<pre>`
 * when a semantic renderer exists for it.
 *
 * Every value shown is a server-projected fact. The inspector computes no
 * durations, infers no outcomes and fills no missing evidence: where the
 * server said a fact is unavailable, this says so too.
 */
import { createContext, useContext, useEffect, useRef, type ReactNode, type RefObject } from 'react';
import type {
  TraceArtifact,
  TraceContentBlock,
  TraceContextKind,
  TraceContextPresentation,
  TraceDetail,
  TraceGeneration,
  TraceJson,
  TraceRecord,
  TraceSystemPromptPresentation,
  TraceText,
  TraceToolDefinition,
} from '../../../../protocol/app-server/v23';
import { writeClipboard } from '../../presentation/primitives/clipboard';
import { Button } from '../../presentation/primitives/Button';
import { JsonTree, type JsonTreeLabels } from '../../presentation/primitives/JsonTree';
import { MarkdownText } from '../../presentation/markdown/MarkdownText';
import { CodeBlock } from '../../presentation/markdown/CodeBlock';
import { Tabs, TabList, Tab, TabPanel } from 'react-aria-components';
import { diffLines } from 'diff';
import type { StructuralDisplayItem, TrajectoryFacet, TrajectorySelection } from './layout';
import { Artifact } from '../components/Artifact';
import { formatDuration, formatInstant } from './timeline';
import css from './Trajectory.module.css';
import { previewOf, cellLabel } from './TrajectoryCell';

function JSON_LABELS(tx: Translate): JsonTreeLabels { return {
  copyValue: tx('trajectory:copy.copy-value'),
  copyJson: tx('trajectory:copy.copy-json'),
  copyPath: tx('trajectory:copy.copy-path'),
  copyPrettyJson: tx('trajectory:copy.copy-pretty-json'),
  copyCompactJson: tx('trajectory:copy.copy-compact-json'),
  copied: tx('trajectory:trajectory-inspector.copied'),
  copyFailed: tx('trajectory:copy.copy-failed'),
  collapseNode: tx('trajectory:trajectory.collapse'),
  expandNode: tx('trajectory:trajectory.expand'),
  copyButtonTitle: action => action,
}; }

/** A large integer the wire carries losslessly as a string. */
function count(value: string | number | null | undefined): number | undefined {
  if (value == null) return undefined;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

function Unavailable() {
  const tx = useTranslation();
  return <span className={css.unavailable}>{tx('trajectory:trajectory-inspector.unavailable')}</span>;
}

function Truncated({ of }: { of: boolean | undefined }) {
  const tx = useTranslation();
  return of ? <p className={css.truncated}>{tx('trajectory:trajectory-inspector.shown-partially-truncated-at-the-inspection-bound')}</p> : null;
}

function Text({ value, markdown = false }: { value: TraceText; markdown?: boolean }) {
  const tx = useTranslation();
  const [copied, setCopied] = useNotice();
  if (value.text === '' && !value.truncated) return <span className={css.unavailable}>{tx('trajectory:trajectory-inspector.empty')}</span>;
  return (
    <>
      {markdown ? (
        <div className={css.markdown}>
          <MarkdownText text={value.text} />
        </div>
      ) : (
        <pre className={css.payload}>{value.text}</pre>
      )}
      <Button size="sm" className={css.copyText} onClick={() => { void writeClipboard(value.text).then(ok => setCopied(ok ? message('trajectory:trajectory-inspector.copied') : message('trajectory:copy.copy-failed'))); }}>{copied || tx('trajectory:trajectory-inspector.copy-text')}</Button>
      <Truncated of={value.truncated} />
    </>
  );
}

/** The inspector body: the scrolling tab panel that keeps the keyboard when a
 * JSON row scrolls out from under its open copy menu. */
const InspectorBody = createContext<RefObject<HTMLElement | null> | undefined>(undefined);

/** The inspector's scrolling tab panel, named as the focus owner of the JSON
 * copy menus inside it. */
function InspectorPanel({ active, children, selectedId, content }: { active: string; children: ReactNode; selectedId?: string; content?: TraceDetail }) {
  const body = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (active !== 'Context' || !selectedId || !body.current) return;
    const target = Array.from(body.current.querySelectorAll<HTMLElement>('[data-context-message-id]')).find(node => node.dataset.contextMessageId === selectedId);
    if (target) body.current.scrollTop += target.getBoundingClientRect().top - body.current.getBoundingClientRect().top;
  }, [active, selectedId, content]);
  return (
    <TabPanel
      ref={body}
      id={active}
      className={css.inspectorBody}
    >
      <InspectorBody value={body}>{children}</InspectorBody>
    </TabPanel>
  );
}

function Structured({ value, label }: { value: TraceJson; label: string }) {
  const tx = useTranslation();
  const body = useContext(InspectorBody);
  const data = value.value;
  if (data === null || typeof data !== 'object') {
    return (
      <>
        <pre className={css.payload}>{JSON.stringify(data)}</pre>
        <Truncated of={value.truncated} />
      </>
    );
  }
  return (
    <>
      <JsonTree
        data={data as object}
        label={label}
        labels={JSON_LABELS(tx)}
        collapsedStringLines={12}
        className={css.jsonTree}
        menuFocusOwner={body}
      />
      <Truncated of={value.truncated} />
    </>
  );
}

function Attachments({ artifacts }: { artifacts: readonly TraceArtifact[] }) {
  const tx = useTranslation();
  if (artifacts.length === 0) return <p className={css.unavailable}>{tx('trajectory:trajectory-inspector.no-durable-artifacts-recorded')}</p>;
  return (
    <div className="attachment-gallery">
      {artifacts.map(artifact => (
        <Artifact
          key={artifact.artifact_id}
          id={artifact.artifact_id}
          name={artifact.name ?? artifact.artifact_id}
          image={artifact.image}
          mimeType={artifact.mime_type ?? undefined}
        />
      ))}
    </div>
  );
}

/** One projected content block, rendered by its own semantic. */
function Block({ block }: { block: TraceContentBlock }): ReactNode {
  const tx = useTranslation();
  switch (block.type) {
    case 'text':
      return <Text value={block.text} markdown />;
    case 'refusal':
      return (
        <div>
          <h4 className={css.blockLabel}>{tx('trajectory:trajectory-inspector.refusal')}</h4>
          <Text value={block.text} markdown />
        </div>
      );
    case 'reasoning':
      return (
        <details className={css.reasoning}>
          <summary>{tx('trajectory:trajectory-inspector.reasoning')}</summary>
          <Text value={block.text} markdown />
        </details>
      );
    case 'json':
      return <Structured value={block.value} label={tx('trajectory:trajectory-inspector.structured-content')} />;
    case 'tool_call':
      return (
        <div className={css.blockGroup}>
          <h4 className={css.blockLabel}>
            {tx('trajectory:trajectory-inspector.proposed-toolcall')}{' '}<span className={css.machine}>{block.name}</span>
          </h4>
          <dl className={css.facts}>
            <dt>{tx('trajectory:trajectory-inspector.toolcall')}</dt>
            <dd className={css.machine}>{block.call_id}</dd>
            <dt>{tx('trajectory:trajectory-inspector.tool')}</dt>
            <dd className={css.machine}>{block.tool_id}</dd>
          </dl>
          <Structured value={block.arguments} label={tx('trajectory:trajectory-inspector.value-arguments', { p0: block.name })} />
        </div>
      );
    case 'tool_result':
      return (
        <div className={css.blockGroup}>
          <h4 className={css.blockLabel}>
            {tx('trajectory:trajectory-inspector.tool-result')}{' '}<span className={css.machine}>{block.outcome}</span>
          </h4>
          {block.blocks.map((nested, index) => (
            <Block key={index} block={nested} />
          ))}
          <Truncated of={block.truncated} />
        </div>
      );
    case 'image':
      return (
        <Attachments artifacts={[{ ...block.artifact, name: block.alt ?? block.artifact.name }]} />
      );
    case 'file':
      return <Attachments artifacts={[block.artifact]} />;
    case 'upload':
      return (
        <p className={css.blockLabel}>
          {tx('trajectory:trajectory-inspector.session-upload')}{' '}<span className={css.machine}>{block.name}</span>
        </p>
      );
  }
}

function Definition({ definition }: { definition: TraceToolDefinition }) {
  const tx = useTranslation();
  return (
    <div className={css.blockGroup}>
      <h4 className={css.blockLabel}>
        {definition.name} <span className={css.machine}>{definition.tool_id}</span>
      </h4>
      <Text value={definition.description} />
      <Structured value={definition.input_schema} label={tx('trajectory:trajectory-inspector.value-input-schema', { p0: definition.name })} />
    </div>
  );
}

/** Derived generation metrics, each unavailable unless its evidence exists. */
function Generation({ generation }: { generation: TraceGeneration }) {
  const tx = useTranslation();
  const ttft = count(generation.ttft_ms);
  const decode = count(generation.generation_ms);
  const rate = generation.output_tokens_per_second ?? undefined;
  return (
    <>
      <dt>{tx('trajectory:trajectory-inspector.request-duration-start-provider-terminal')}</dt>
      <dd>{formatDuration(tx, count(generation.timeline?.terminal_ms))}</dd>
      <dt>{tx('trajectory:trajectory-inspector.request-start-dispatch')}</dt>
      <dd>{formatDuration(tx, count(generation.timeline?.dispatch_ms))}</dd>
      <dt>{tx('trajectory:trajectory-inspector.dispatch-first-output-ttft')}</dt>
      <dd>{ttft === undefined ? <Unavailable /> : formatDuration(tx, ttft)}</dd>
      <dt>{tx('trajectory:trajectory-inspector.first-output-provider-terminal')}</dt>
      <dd>{decode === undefined ? <Unavailable /> : formatDuration(tx, decode)}</dd>
      <dt>{tx('trajectory:trajectory-inspector.dispatch-provider-terminal')}</dt>
      <dd>{formatDuration(tx, count(generation.terminal_ms))}</dd>
      <dt>{tx('trajectory:trajectory-inspector.throughput')}</dt>
      <dd>{rate === undefined ? <Unavailable /> : tx('trajectory:trajectory-inspector.value-tokens-s', { p0: rate.toFixed(1) })}</dd>
    </>
  );
}

/**
 * What the server's System Prompt classification means, in words.
 *
 * Each sentence states the native authority behind the classification,
 * because the difference that matters to a reader is *what was compared*:
 * the nearest preceding actual request, not the previous row on screen.
 */
function SYSTEM_PROMPT_STATE(tx: Translate): Record<
  TraceSystemPromptPresentation['state'],
  { label: string; note: string }
> { return {
  initial: {
    label: tx('trajectory:trajectory-inspector.initial'),
    note: tx('trajectory:copy.no-earlier-actual-request-exists-in-this-conversation'),
  },
  changed: {
    label: tx('trajectory:trajectory-inspector.changed'),
    note: tx('trajectory:copy.the-nearest-preceding-actual-request-froze-a-different-prompt'),
  },
  unchanged: {
    label: tx('trajectory:trajectory-inspector.unchanged'),
    note: tx('trajectory:copy.the-nearest-preceding-actual-request-froze-the-identical-prompt'),
  },
  previous_unavailable: {
    label: tx('trajectory:trajectory-inspector.previous-unavailable'),
    note: tx('trajectory:copy.a-preceding-request-exists-but-its-frozen-prompt-could-not-be-established-at-this-read-cut'),
  },
}; }

/** Display names for the closed Context presentation families. */
const CONTEXT_KIND: Record<TraceContextKind, TranslationKey> = { native_environment: "trajectory:context.native_environment", goal_status: "trajectory:context.goal_status", runtime_tool_observation: "trajectory:context.runtime_tool_observation", extension_environment: "trajectory:context.extension_environment", agent_status: "trajectory:context.agent_status" };

/**
 * The exact native producer the server copied from the canonical message.
 *
 * Two certified extensions publish the same Context family, so the family
 * cannot name the producer and this renders the contributor identity the
 * server sent. No name is derived from the Context kind, and no extension
 * catalog is consulted: this is display of a resolved fact, not inference.
 */
function contextSource(tx: Translate, source: TraceContextPresentation['source']): string {
  return source.type === 'runtime' ? tx('trajectory:copy.runtime') : tx('trajectory:copy.extension-value', { p0: source.contributor });
}

/**
 * The System Prompt relationship the server resolved for this request.
 *
 * Nothing here compares request details. The classification, and the page
 * independence that makes it trustworthy, are native facts; this renders
 * them and names the complete prompt's own home.
 */
function SystemPrompt({ system }: { system: TraceSystemPromptPresentation }) {
  const tx = useTranslation();
  const { label, note } = SYSTEM_PROMPT_STATE(tx)[system.state];
  return (
    <>
      <dt>{tx('trajectory:trajectory-inspector.system-prompt')}</dt>
      <dd>
        {label}
        <p className={css.note}>{note}</p>
        {system.preview && (
          <>
            <p className={css.machine}>{system.preview.text}</p>
            <Truncated of={system.preview.truncated} />
          </>
        )}
      </dd>
    </>
  );
}

/**
 * Canonical Context this request introduced, in the server's frozen order.
 *
 * The order is `RequestSnapshot.request_context_ids`, so it is rendered as
 * given: no sort by time, family, provenance or label happens here.
 */
function ContextAdditions({
  additions,
  truncated,
  selectedId,
}: {
  selectedId?: string | undefined;
  additions: readonly TraceContextPresentation[];
  truncated: boolean;
}) {
  const tx = useTranslation();
  return (
    <>
      <h3 className={css.sectionLabelHeading}>{tx('trajectory:trajectory-inspector.context-introduced-by-this-request')}</h3>
      <p className={css.note}>
        {tx('trajectory:trajectory-inspector.the-canonical-context-facts-this-actual-request-committed-with-i')}</p>
      {additions.length === 0 && (
        <p className={css.unavailable}>{tx('trajectory:trajectory-inspector.this-request-introduced-no-canonical-context')}</p>
      )}
      {additions.map(addition => (
        <section key={addition.message_id} className={css.requestMessage} data-context-message-id={addition.message_id} data-selected={addition.message_id === selectedId || undefined}>
          <h4 className={css.blockLabel}>
            {tx(CONTEXT_KIND[addition.context_kind])} · {contextSource(tx, addition.source)}
            <span className={css.machine}> {addition.message_id}</span>
          </h4>
          {addition.preview ? <p>{addition.preview.text || tx('trajectory:trajectory-inspector.empty')}</p> : <p>{tx('trajectory:trajectory-inspector.content-unavailable')}</p>}
          {addition.attachments.length > 0 && <Attachments artifacts={addition.attachments} />}
          <Truncated of={addition.truncated || addition.preview?.truncated} />
        </section>
      ))}
      {truncated && (
        <p className={css.truncated}>
          {tx('trajectory:trajectory-inspector.further-context-facts-omitted-at-the-summary-bound-the-first-in')}</p>
      )}
    </>
  );
}

/** The sections available for one record, given what the server projected. */
function sectionsOf(record: TraceRecord, detail: TraceDetail | undefined, selection: TrajectorySelection): TrajectoryFacet[] {
  if (record.kind === 'request') {
    const diff: TrajectoryFacet[] = record.request?.system_prompt.state === 'changed' ? ['Diff'] : [];
    if (selection.cell_type === 'SystemPromptCell') return [...diff, 'System Prompt', 'Tools', 'Summary', 'Native'];
    if (selection.cell_type === 'ContextRow') return ['Context', 'Summary', 'Native'];
    return ['Summary', 'System Prompt', ...diff, 'Context', 'Tools', 'Options', 'Usage', 'Timing', 'Native'];
  }
  if (record.kind === 'assistant') return ['Summary', 'Content', ...(detail?.messages.some(message => message.blocks.some(block => block.type === 'reasoning')) ? ['Thinking' as const] : []), 'Raw', 'Timing', 'Native'];
  if (record.kind === 'tool') return ['Summary', 'Input', ...(detail?.tool?.source ? ['Code' as const] : []), 'Result', 'Schema', 'Timing', 'Artifacts', 'Native'];
  return ['Summary', ...(detail?.messages.length ? ['Content' as const] : []), 'Timing', 'Artifacts', 'Native'];
}

/** Native classification is an input, never the output of jsdiff. */
function PromptDiff({ record, detail }: { record: TraceRecord; detail: TraceDetail | undefined }) {
  const tx = useTranslation();
  const request = detail?.request;
  if (!request) return <p className={css.unavailable}>{tx('trajectory:trajectory-inspector.prompt-content-unavailable-until-detail-is-loaded')}</p>;
  const previous = request.previous_system_prompt;
  if (request.predecessor.availability === 'not_applicable') return <p>{tx('trajectory:trajectory-inspector.no-predecessor-initial-prompt')}</p>;
  if (request.predecessor.availability === 'unavailable' || !previous) return <p>{tx('trajectory:trajectory-inspector.previous-prompt-unavailable-a-complete-diff-cannot-be-produced')}</p>;
  if (request.effective_system_prompt.truncated || previous.truncated) return <p className={css.truncated}>{tx('trajectory:trajectory-inspector.a-complete-diff-cannot-be-produced')}{' '}{request.effective_system_prompt.truncated ? tx('trajectory:trajectory-inspector.current-prompt-truncated') : ''}{request.effective_system_prompt.truncated && previous.truncated ? '; ' : ''}{previous.truncated ? tx('trajectory:trajectory-inspector.previous-prompt-truncated') : ''}{tx('trajectory:trajectory-inspector.native-relationship')}{' '}{record.request?.system_prompt.state}.</p>;
  if (record.request?.system_prompt.state === 'unchanged') return <p>{tx('trajectory:trajectory-inspector.no-changes-complete-frozen-prompts-are-natively-unchanged')}</p>;
  if (record.request?.system_prompt.state !== 'changed') return <p>{tx('trajectory:trajectory-inspector.native-prompt-relationship-unavailable-a-complete-diff-cannot-be')}</p>;
  const changes = diffLines(previous.text, request.effective_system_prompt.text, { maxEditLength: 4096 });
  if (!changes) return <p>{tx('trajectory:trajectory-inspector.a-complete-diff-cannot-be-produced-within-the-display-work-bound')}</p>;
  return <pre className={css.diff} aria-label={tx('trajectory:trajectory-inspector.system-prompt-diff')}>{changes.map((change, index) => <span key={index} data-change={change.added ? 'added' : change.removed ? 'removed' : 'context'}>{change.added ? '+ ' : change.removed ? '− ' : '  '}{change.value}</span>)}</pre>;
}

type ToolDetailState =
  | { type: 'pending'; loading: boolean }
  | { type: 'read_error'; error: string }
  | { type: 'loaded_missing_tool' }
  | { type: 'loaded_tool' };

/** Only a successful historical read can establish payload or fact absence. */
function toolDetailState(detail: TraceDetail | undefined, loading: boolean | undefined, error: string | undefined): ToolDetailState {
  if (error) return { type: 'read_error', error };
  if (loading || !detail) return { type: 'pending', loading: loading === true };
  return { type: detail.tool ? 'loaded_tool' : 'loaded_missing_tool' };
}
function ToolFacet({ state, facet, children }: { state: ToolDetailState; facet: string; children: ReactNode }) {
  const tx = useTranslation();
  switch (state.type) {
    case 'pending': return <p role="status" className={css.unavailable}>{state.loading ? tx('trajectory:trajectory-inspector.loading-record-detail') : tx('trajectory:trajectory-inspector.historical-tool-detail-has-not-been-loaded')}</p>;
    case 'read_error': return <p role="alert" className={css.error}>{facet} {tx('trajectory:trajectory-inspector.could-not-be-established-because-the-historical-detail-read-fail')}{' '}{state.error}</p>;
    case 'loaded_missing_tool': return <p className={css.unavailable}>{tx('trajectory:trajectory-inspector.tool-detail-is-unavailable-in-this-bounded-detail-projection')}</p>;
    case 'loaded_tool': return children;
  }
}

/** Props for the Trajectory record inspector. */
export interface TrajectoryInspectorProps {
  record: TraceRecord;
  selection: TrajectorySelection;
  onFacet: (facet: TrajectoryFacet) => void;
  detail?: TraceDetail | undefined;
  loading?: boolean | undefined;
  error?: string | undefined;
  /** Request the heavy detail of this record; the owner fences the reply. */
  onLoadDetail: (id: string) => void;
  onClose: () => void;
}

/**
 * Render the inspector for one selected record.
 * @param props - the selected record, its fetched detail, and load controls.
 * @returns the inspector panel.
 */
export function TrajectoryInspector({
  record,
  selection,
  onFacet,
  detail,
  loading,
  error,
  onLoadDetail,
  onClose,
}: TrajectoryInspectorProps) {
  const tx = useTranslation();
  const section = selection.facet;
  useEffect(() => {
    if (record.has_detail && !detail && !loading && !error) onLoadDetail(record.id);
  }, [record.id, record.has_detail, detail, loading, error, onLoadDetail]);
  const sections = sectionsOf(record, detail, selection);
  const active = sections.includes(section) ? section : 'Summary';
  const request = detail?.request ?? undefined;
  const tool = detail?.tool ?? undefined;
  const toolState = toolDetailState(detail, loading, error);
  const toolFactFacet = record.kind === 'tool' && ['Input', 'Result', 'Schema'].includes(active);
  const messages = detail?.messages ?? [];
  const title = selection.cell_type === 'SystemPromptCell' ? tx('trajectory:copy.system-prompt') : selection.cell_type === 'ContextRow' ? tx('trajectory:copy.context') :
    record.kind === 'request' && record.request
      ? tx('trajectory:copy.request-value', { p0: record.request.model })
      : record.kind === 'tool' && record.tool
        ? tx('trajectory:copy.tool-value', { p0: record.tool.name ?? record.tool.tool_id })
        : cellLabel(tx)[record.kind];

  return (
    <aside className={css.inspector} aria-label={tx('trajectory:trajectory-inspector.trace-record-inspector')}>
      <header>
        <strong>{title}</strong>
        <Button size="sm" onClick={onClose}>
          {tx('trajectory:trajectory-inspector.close-record')}</Button>
      </header>
      <Tabs className={css.inspectorTabs} selectedKey={active} onSelectionChange={key => onFacet(key as TrajectoryFacet)}>
      <TabList aria-label={tx('trajectory:trajectory-inspector.record-sections')} className={css.tabs}>
        {sections.map(name => <Tab key={name} id={name}>{tx(`trajectory:facet.${name}`)}</Tab>)}
      </TabList>
      {loading && !toolFactFacet && (
        <p role="status" className={css.unavailable}>
          {tx('trajectory:trajectory-inspector.loading-record-detail')}</p>
      )}
      {error && !toolFactFacet && (
        <p role="alert" className={css.error}>
          {error}
        </p>
      )}
      <InspectorPanel active={active} selectedId={selection.context_message_id} content={detail}>
        {active === 'Summary' && (
          <>
          <div className={css.summaryPreview}><MarkdownText text={previewOf(tx, record)} /></div>
          {tool?.source && <section className={css.summaryPreview}>
            <h3 className={css.sectionLabelHeading}>{tx('trajectory:trajectory-inspector.code')}</h3>
            <CodeBlock code={tool.source.text.text} lang={tool.source.language ?? undefined} lineNumbers copyLabel={tx('trajectory:copy.copy-source')} copiedLabel={tx('trajectory:trajectory-inspector.copied')} />
            <Truncated of={tool.source.text.truncated} />
          </section>}
          {tool?.arguments && !tool.source && <section className={css.summaryPreview}>
            <h3 className={css.sectionLabelHeading}>{tx('trajectory:trajectory-inspector.input')}</h3><Structured value={tool.arguments} label={tx('trajectory:trajectory-inspector.recorded-arguments')} />
          </section>}
          {tool?.result && <section className={css.summaryPreview}>
            <h3 className={css.sectionLabelHeading}>{tx('trajectory:trajectory-inspector.result')}{' '}{tool.result.outcome}</h3>
            {tool.result.blocks.map((block, index) => <Block key={index} block={block} />)}
            <Truncated of={tool.result.blocks_truncated} />
          </section>}
          <dl className={css.facts}>
            <dt>{tx('trajectory:trajectory-inspector.status')}</dt><dd>{record.state}</dd>
            {record.request && <>
              <dt>{tx('trajectory:trajectory-inspector.model')}</dt><dd>{record.request.model}</dd>
              <dt>{tx('trajectory:trajectory-inspector.retry-recovery-ordinal')}</dt><dd>{record.request.retry_number}</dd>
              <SystemPrompt system={record.request.system_prompt} />
              <dt>{tx('trajectory:trajectory-inspector.tools')}</dt><dd>{record.request.tool_catalog.replaceAll('_', ' ')}</dd>
              <dt>{tx('trajectory:trajectory-inspector.context-introduced')}</dt><dd>{record.request.context_additions.length}{record.request.context_truncated ? tx('trajectory:trajectory-inspector.truncated') : ''}</dd>
              {record.request.failure_kind && <><dt>{tx('trajectory:trajectory-inspector.failure')}</dt><dd>{record.request.failure_kind}</dd></>}
              {sections.includes('System Prompt') && <><dt>{tx('trajectory:trajectory-inspector.historical-input')}</dt><dd><Button size="sm" onClick={() => onFacet('System Prompt')}>{tx('trajectory:trajectory-inspector.view-system-prompt')}</Button> <Button size="sm" onClick={() => onFacet('Tools')}>{tx('trajectory:trajectory-inspector.view-tools')}</Button></dd></>}
              <dt>{tx('trajectory:trajectory-inspector.acceptance')}</dt><dd>{tx('trajectory:trajectory-inspector.provider-completion-alone-does-not-prove-canonical-assistant-acc')}</dd>
            </>}
            {record.calls.length > 0 && <><dt>{tx('trajectory:trajectory-inspector.proposed-calls')}</dt><dd>{record.calls.length} {tx('trajectory:trajectory-inspector.a-proposal-proves-assembly-not-execution')}</dd></>}
            {record.tool && <><dt>{tx('trajectory:trajectory-inspector.execution')}</dt><dd>{record.tool.started ? tx('trajectory:trajectory-inspector.started-a-durable-start-fact-exists') : tx('trajectory:trajectory-inspector.proposed-only')}</dd><dt>{tx('trajectory:trajectory-inspector.outcome')}</dt><dd>{record.tool.outcome ?? tx('trajectory:trajectory-inspector.unknown')}</dd></>}
          </dl>
          <Truncated of={record.truncated || detail?.truncated} />

          </>
        )}

        {active === 'Native' && <>          <section className={css.nativeDetails}>
          <dl className={css.facts}>
            <dt>{tx('trajectory:trajectory-inspector.state')}</dt>
            <dd>{record.state}</dd>
            <dt>{tx('trajectory:trajectory-inspector.record')}</dt>
            <dd className={css.machine}>{record.id}</dd>
            <dt>{tx('trajectory:trajectory-inspector.attempt')}</dt>
            <dd className={css.machine}>{record.location.attempt_id ?? <Unavailable />}</dd>
            <dt>{tx('trajectory:trajectory-inspector.logical-step')}</dt>
            <dd className={css.machine}>{record.location.step_id ?? <Unavailable />}</dd>
            {record.request && (
              <>
                <dt>{tx('trajectory:trajectory-inspector.actual-request')}</dt>
                <dd className={css.machine}>{record.request.request_id}</dd>
                <dt>{tx('trajectory:trajectory-inspector.retry-recovery-ordinal')}</dt>
                <dd>
                  {record.request.retry_number}
                  {record.request.retry_number > 0
                    ? tx('trajectory:trajectory-inspector.retry-or-recovery-within-this-step')
                    : tx('trajectory:trajectory-inspector.initial-request')}
                </dd>
                <dt>{tx('trajectory:trajectory-inspector.preceding-request-failure')}</dt>
                <dd>{record.request.previous_failure_kind ?? <Unavailable />}</dd>
                <dt>{tx('trajectory:trajectory-inspector.failure-class')}</dt>
                <dd>{record.request.failure_kind ?? tx('trajectory:trajectory-inspector.none-recorded')}</dd>
                <SystemPrompt system={record.request.system_prompt} />
                <dt>{tx('trajectory:trajectory-inspector.context-introduced')}</dt>
                <dd>
                  {record.request.context_additions.length}
                  {record.request.context_truncated ? tx('trajectory:trajectory-inspector.bounded') : ''}
                </dd>
              </>
            )}
            {record.originating_tool_call_id && (
              <>
                <dt>{tx('trajectory:trajectory-inspector.originating-toolcall')}</dt>
                <dd className={css.machine}>{record.originating_tool_call_id}</dd>
                <dd className={css.note}>
                  {tx('trajectory:trajectory-inspector.server-resolved-navigation-correlation-from-this-domain-s-own-st')}</dd>
              </>
            )}
            {record.tool && (
              <>
                <dt>{tx('trajectory:trajectory-inspector.toolcall')}</dt>
                <dd className={css.machine}>{record.tool.call_id}</dd>
                <dt>{tx('trajectory:trajectory-inspector.tool')}</dt>
                <dd className={css.machine}>{record.tool.tool_id}</dd>
                <dt>{tx('trajectory:trajectory-inspector.execution')}</dt>
                <dd>
                  {tool?.lifecycle === 'settled'
                    ? tx('trajectory:trajectory-inspector.settled-a-canonical-tool-message-was-accepted')
                    : record.tool.started
                      ? tx('trajectory:trajectory-inspector.started-a-durable-start-fact-exists')
                      : tx('trajectory:trajectory-inspector.proposed-assembly-only-execution-is-not-implied')}
                </dd>
                <dt>{tx('trajectory:trajectory-inspector.outcome')}</dt>
                <dd>{record.tool.outcome ?? <Unavailable />}</dd>
                {record.tool.detail && (
                  <>
                    <dt>{tx('trajectory:trajectory-inspector.detail')}</dt>
                    <dd>{record.tool.detail.text}</dd>
                  </>
                )}
              </>
            )}
            {record.calls.length > 0 && (
              <>
                <dt>{tx('trajectory:trajectory-inspector.assembled-toolcalls')}</dt>
                <dd>
                  {record.calls.map(call => (
                    <div key={call.call_id} className={css.machine}>
                      {call.name} · {call.call_id}
                    </div>
                  ))}
                  <p className={css.note}>
                    {tx('trajectory:trajectory-inspector.a-proposal-proves-assembly-execution-requires-its-own-started-re')}</p>
                </dd>
              </>
            )}
            {record.native_id && (
              <>
                <dt>{tx('trajectory:trajectory-inspector.native-identity')}</dt>
                <dd className={css.machine}>{record.native_id}</dd>
              </>
            )}
            {record.message_id && (
              <>
                <dt>{tx('trajectory:trajectory-inspector.canonical-message')}</dt>
                <dd className={css.machine}>{record.message_id}</dd>
              </>
            )}
            {record.kind === 'request' && (
              <>
                <dt>{tx('trajectory:trajectory-inspector.acceptance')}</dt>
                <dd className={css.note}>
                  {tx('trajectory:trajectory-inspector.provider-completion-alone-does-not-prove-canonical-assistant-acc')}</dd>
              </>
            )}
            <Truncated of={record.truncated} />
          </dl>
          </section></>}

        {active === 'Content' && messages.map(message => (
          <section key={message.message_id}>
            <dl className={css.facts}>
              <dt>{tx('trajectory:trajectory-inspector.role')}</dt>
              <dd>{message.role}</dd>
              {message.source && (
                <>
                  <dt>{tx('trajectory:trajectory-inspector.provenance')}</dt>
                  <dd>{message.source}</dd>
                </>
              )}
            </dl>
            {message.blocks.map((block, index) => (
              <Block key={index} block={block} />
            ))}
            <Truncated of={message.truncated} />
          </section>
        ))}

        {active === 'Raw' && (
          <Structured value={{ value: messages, truncated: detail?.truncated ?? false }} label={tx('trajectory:trajectory-inspector.projected-messages')} />
        )}

        {active === 'System Prompt' && request && (<><h3 className={css.sectionLabelHeading}>{tx('trajectory:trajectory-inspector.effective-system-prompt')}</h3><Text value={request.effective_system_prompt} markdown /></>)}

        {active === 'Diff' && <PromptDiff record={record} detail={detail} />}

        {active === 'Thinking' && messages.map(message => <section key={message.message_id}>{message.blocks.filter(block => block.type === 'reasoning').map((block, index) => <Text key={index} value={block.text} markdown />)}</section>)}

        {active === 'Context' && record.request && (
          <ContextAdditions
            additions={record.request.context_additions}
            truncated={record.request.context_truncated}
            selectedId={selection.context_message_id}
          />
        )}

        {active === 'Context' && request && (
          <>
            {request.contributions.map(contribution => (
              <section key={contribution.message_id} className={css.requestMessage}>
                <h3 className={css.sectionLabelHeading}>{tx('trajectory:trajectory-inspector.accepted-contribution')}</h3>
                <p className={css.note}>{tx('trajectory:trajectory-inspector.frozen-request-context-not-current-domain-state')}</p>
                <Structured value={{ value: contribution, truncated: false }} label={tx('trajectory:trajectory-inspector.accepted-contribution-metadata')} />
              </section>
            ))}
            <h3 className={css.sectionLabelHeading}>{tx('trajectory:trajectory-inspector.reconstructed-request-context')}</h3>
            <p className={css.note}>
              {tx('trajectory:trajectory-inspector.the-exact-provider-neutral-messages-this-request-carried-rebuilt')}</p>
            {request.messages.map((entry, index) => (
              <section key={entry.message_id ?? index} className={css.requestMessage} data-context-message-id={entry.message_id} data-selected={entry.message_id === selection.context_message_id || undefined}>
                <h4 className={css.blockLabel}>
                  {entry.role}
                  {entry.source ? tx('trajectory:trajectory-inspector.value', { p0: entry.source }) : ''}
                  {entry.message_id ? (
                    <span className={css.machine}> {entry.message_id}</span>
                  ) : (
                    <span className={css.note}> {tx('trajectory:trajectory-inspector.request-only-no-canonical-identity')}</span>
                  )}
                </h4>
                {entry.blocks.map((block, blockIndex) => (
                  <Block key={blockIndex} block={block} />
                ))}
                <Truncated of={entry.truncated} />
              </section>
            ))}
            {request.messages_truncated && (
              <p className={css.truncated}>
                {tx('trajectory:trajectory-inspector.older-context-omitted-at-the-inspection-bound-the-newest-items-a')}</p>
            )}
          </>
        )}

        {active === 'Input' && (
          <ToolFacet state={toolState} facet={active}>
            <h3 className={css.sectionLabelHeading}>{tx('trajectory:trajectory-inspector.recorded-arguments')}</h3>
            {tool?.arguments ? (
              <Structured value={tool.arguments} label={tx('trajectory:trajectory-inspector.value-arguments', { p0: tool.name ?? tool.tool_id })} />
            ) : (
              <p className={css.unavailable}>
                {tx('trajectory:trajectory-inspector.the-canonical-proposal-for-this-call-is-not-loadable-at-this-rea')}</p>
            )}
          </ToolFacet>
        )}

        {active === 'Code' && tool?.source && (
          <>
            <h3 className={css.sectionLabelHeading}>
              {tx('trajectory:trajectory-inspector.program-source')}{' '}<span className={css.machine}>{tool.source.field}</span>
            </h3>
            <p className={css.note}>
              {tool.source.language
                ? tx('trajectory:trajectory-inspector.highlighted-as-value-the-native-tool-contract-fixes-this-languag', { p0: tool.source.language })
                : tx('trajectory:trajectory-inspector.no-language-is-highlighted-this-tool-contract-identifies-the-fie')}
            </p>
            <CodeBlock
              code={tool.source.text.text}
              lang={tool.source.language ?? undefined}
              lineNumbers
              copyLabel={tx('trajectory:copy.copy-source')}
              copiedLabel={tx('trajectory:trajectory-inspector.copied')}
            />
            <Truncated of={tool.source.text.truncated} />
          </>
        )}

        {active === 'Result' && <ToolFacet state={toolState} facet={active}>
          {tool?.result ? <>
            <dl className={css.facts}>
              <dt>{tx('trajectory:trajectory-inspector.outcome')}</dt>
              <dd>{tool.result.outcome}</dd>
              <dt>{tx('trajectory:trajectory-inspector.execution-duration')}</dt>
              <dd>{formatDuration(tx, count(tool.result.duration_ms))}</dd>
              {tool.result.exit_code != null && (
                <>
                  <dt>{tx('trajectory:trajectory-inspector.exit-code')}</dt>
                  <dd className={css.machine}>{tool.result.exit_code}</dd>
                </>
              )}
              {tool.result.truncation && (
                <>
                  <dt>{tx('trajectory:trajectory-inspector.tool-recorded-truncation')}</dt>
                  <dd>
                    {tool.result.truncation.truncated ? tx('trajectory:trajectory-inspector.output-was-truncated-by-the-tool') : tx('trajectory:trajectory-inspector.complete')}
                    {tool.result.truncation.original_bytes != null
                      ? tx('trajectory:trajectory-inspector.value-bytes-originally', { p0: tool.result.truncation.original_bytes })
                      : ''}
                  </dd>
                </>
              )}
              {tool.result.managed_output && (
                <>
                  <dt>{tx('trajectory:trajectory-inspector.managed-output')}</dt>
                  <dd>
                    {tool.result.managed_output.available
                      ? tool.result.managed_output.complete
                        ? tx('trajectory:trajectory-inspector.complete')
                        : tx('trajectory:trajectory-inspector.partial')
                      : tx('trajectory:trajectory-inspector.unavailable')}
                  </dd>
                  {tool.result.managed_output.locator != null ? (
                    <>
                      <dt>{tx('trajectory:trajectory-inspector.locator')}</dt>
                      <dd>{tool.result.managed_output.locator}</dd>
                    </>
                  ) : null}
                  {tool.result.managed_output.diagnostic ? (
                    <>
                      <dt>{tx('trajectory:trajectory-inspector.diagnostic')}</dt>
                      <dd><Text value={tool.result.managed_output.diagnostic} /></dd>
                    </>
                  ) : null}
                </>
              )}
            </dl>
            {tool.result.detail && (
              <>
                <h3 className={css.sectionLabelHeading}>{tx('trajectory:trajectory-inspector.status-detail')}</h3>
                <Text value={tool.result.detail} />
              </>
            )}
            {tool.result.blocks.map((block, index) => (
              <Block key={index} block={block} />
            ))}
            <Truncated of={tool.result.blocks_truncated} />
          </> : <p className={css.unavailable}>{tx('trajectory:trajectory-inspector.no-canonical-tool-result-is-recorded-at-this-read-cut')}</p>}
        </ToolFacet>}

        {active === 'Schema' && <ToolFacet state={toolState} facet={active}>
          {tool?.definition ? <Definition definition={tool.definition} /> : <p className={css.unavailable}>{tx('trajectory:trajectory-inspector.the-historical-tool-definition-is-unavailable-at-this-read-cut')}</p>}
        </ToolFacet>}

        {active === 'Tools' && request && (
          <>
            <p className={css.note}>
              {tx('trajectory:trajectory-inspector.the-exact-historical-tool-catalog-this-request-carried-not-the-c')}</p>
            {request.tools.length === 0 && <p className={css.unavailable}>{tx('trajectory:trajectory-inspector.no-tool-definitions-recorded')}</p>}
            {request.tools.map(definition => (
              <Definition key={definition.tool_id} definition={definition} />
            ))}
            {request.tools_truncated && (
              <p className={css.truncated}>{tx('trajectory:trajectory-inspector.further-tool-definitions-omitted-at-the-inspection-bound')}</p>
            )}
          </>
        )}

        {active === 'Options' && request && (
          <>
            <dl className={css.facts}>
              <dt>{tx('trajectory:trajectory-inspector.model')}</dt>
              <dd className={css.machine}>{request.model}</dd>
              <dt>{tx('trajectory:trajectory-inspector.protocol')}</dt>
              <dd className={css.machine}>{request.protocol}</dd>
              <dt>{tx('trajectory:trajectory-inspector.output-token-limit')}</dt>
              <dd className={css.machine}>{request.max_output_tokens}</dd>
              <dt>{tx('trajectory:trajectory-inspector.context-window')}</dt>
              <dd className={css.machine}>{request.context_window_tokens}</dd>
              <dt>{tx('trajectory:trajectory-inspector.reasoning')}</dt>
              <dd>
                {request.reasoning_enabled ? tx('trajectory:trajectory-inspector.enabled') : tx('trajectory:trajectory-inspector.disabled')}
                {request.reasoning_profile ? tx('trajectory:trajectory-inspector.value', { p0: request.reasoning_profile }) : ''}
              </dd>
              {request.options.map(option => (
                <div key={option.name} className={css.option}>
                  <dt className={css.machine}>{option.name}</dt>
                  <dd className={css.machine}>{JSON.stringify(option.value.value)}</dd>
                </div>
              ))}
            </dl>
            {request.omitted_option_count > 0 && (
              <p className={css.note}>
                {tx(request.omitted_option_count === 1 ? 'trajectory:options.omitted.one' : 'trajectory:options.omitted.other', { n: request.omitted_option_count })}</p>
            )}
          </>
        )}

        {active === 'Usage' && (
          <dl className={css.facts}>
            {record.request?.usage ? (
              <>
                <dt>{tx('trajectory:trajectory-inspector.input-tokens')}</dt>
                <dd className={css.machine}>{record.request.usage.input_tokens}</dd>
                <dt>{tx('trajectory:trajectory-inspector.output-tokens')}</dt>
                <dd className={css.machine}>{record.request.usage.output_tokens}</dd>
                <dt>{tx('trajectory:trajectory-inspector.total-tokens')}</dt>
                <dd className={css.machine}>{record.request.usage.total_tokens}</dd>
                <dt>{tx('trajectory:trajectory-inspector.reasoning-tokens')}</dt>
                <dd className={css.machine}>
                  {record.request.usage.details?.reasoning_tokens ?? <Unavailable />}
                </dd>
                <dt>{tx('trajectory:trajectory-inspector.cached-input')}</dt>
                <dd className={css.machine}>
                  {record.request.usage.details?.cached_input_tokens ?? <Unavailable />}
                </dd>
              </>
            ) : (
              <>
                <dt>{tx('trajectory:trajectory-inspector.usage')}</dt>
                <dd>
                  <Unavailable />
                </dd>
              </>
            )}
          </dl>
        )}

        {active === 'Timing' && (
          <dl className={css.facts}>
            <dt>{tx('trajectory:trajectory-inspector.started')}</dt>
            <dd className={css.machine}>{formatInstant(tx, record.timing.started_at)}</dd>
            <dt>{tx('trajectory:trajectory-inspector.ended')}</dt>
            <dd className={css.machine}>{formatInstant(tx, record.timing.ended_at)}</dd>
            <dt>{record.kind === 'request' ? tx('trajectory:trajectory-inspector.journal-wall-duration') : tx('trajectory:trajectory.duration')}</dt>
            <dd>{record.timing.duration_ms == null ? <Unavailable /> : formatDuration(tx, count(record.timing.duration_ms))}</dd>
            {record.request?.generation ? (
              <Generation generation={record.request.generation} />
            ) : record.kind === 'request' ? (
              <>
                <dt>{tx('trajectory:trajectory-inspector.generation-evidence')}</dt>
                <dd className={css.note}>
                  {tx('trajectory:trajectory-inspector.no-settled-generation-evidence-was-recorded-for-this-request')}</dd>
              </>
            ) : null}
            <dt>{tx('trajectory:trajectory-inspector.source')}</dt>
            <dd className={css.note}>
              {record.timing.duration_ms == null
                ? tx('trajectory:trajectory-inspector.one-endpoint-only-an-in-flight-or-unterminated-record-has-no-dur')
                : tx('trajectory:trajectory-inspector.two-authoritative-durable-timestamps')}
            </dd>
          </dl>
        )}

        {active === 'Artifacts' && (
          <Attachments artifacts={[...record.attachments, ...(tool?.result?.attachments ?? [])]} />
        )}
      </InspectorPanel>
      </Tabs>
    </aside>
  );
}

/**
 * Bounded evidence of one Turn or Step header.
 *
 * A header is presentation structure, not a detail owner: this reads only the
 * exact native Attempt/Step summary record the projection already attached to
 * it, and issues no detail read. Without that exact record it says so rather
 * than borrowing identity, lifecycle or timing from a member record.
 */
export function TrajectoryStructureInspector({ item, onClose }: { item: StructuralDisplayItem; onClose: () => void }) {
  const tx = useTranslation();
  const record = item.native_record;
  const native = item.type === 'TurnHeader' ? tx('trajectory:trajectory-inspector.attempt') : item.kind === 'step' ? tx('trajectory:copy.step') : undefined;
  return (
    <aside className={css.inspector} aria-label={tx('trajectory:copy.trace-structure-inspector')}>
      <header>
        <strong>{item.label}{native ? ' ' + tx('trajectory:copy.native-value', { p0: native }) : ''}</strong>
        <Button size="sm" onClick={onClose}>{tx('trajectory:copy.close-structure')}</Button>
      </header>
      <div className={css.inspectorBody}>
        <p className={css.note}>“{item.label}{tx('trajectory:copy.is-a-loaded-window-ordinal-not-an-identity')}</p>
        {record ? (
          <>
            <dl className={css.facts}>
              <dt>{tx('trajectory:copy.native-kind')}</dt>
              <dd>{cellLabel(tx)[record.kind]}</dd>
              <dt>{tx('trajectory:trajectory-inspector.state')}</dt>
              <dd>{record.state}</dd>
              <dt>{tx('trajectory:trajectory-inspector.record')}</dt>
              <dd className={css.machine}>{record.id}</dd>
              <dt>{tx('trajectory:trajectory-inspector.attempt')}</dt>
              <dd className={css.machine}>{record.location.attempt_id ?? <Unavailable />}</dd>
              {record.kind === 'step' && (
                <>
                  <dt>{tx('trajectory:trajectory-inspector.logical-step')}</dt>
                  <dd className={css.machine}>{record.location.step_id ?? <Unavailable />}</dd>
                </>
              )}
              {record.native_id && (
                <>
                  <dt>{tx('trajectory:trajectory-inspector.native-identity')}</dt>
                  <dd className={css.machine}>{record.native_id}</dd>
                </>
              )}
              <dt>{tx('trajectory:trajectory-inspector.started')}</dt>
              <dd className={css.machine}>{formatInstant(tx, record.timing.started_at)}</dd>
              <dt>{tx('trajectory:trajectory-inspector.ended')}</dt>
              <dd className={css.machine}>{formatInstant(tx, record.timing.ended_at)}</dd>
              <dt>{tx('trajectory:trajectory.duration')}</dt>
              <dd>{record.timing.duration_ms == null ? <Unavailable /> : formatDuration(tx, count(record.timing.duration_ms))}</dd>
              {record.preview?.text && (
                <>
                  <dt>{tx('trajectory:copy.summary')}</dt>
                  <dd>{record.preview.text}</dd>
                </>
              )}
            </dl>
            <Truncated of={record.truncated || record.preview?.truncated} />
          </>
        ) : (
          <p className={css.unavailable}>
            {native
              ? tx('trajectory:copy.the-exact-native-value-record-is-not-loaded-at-this-read-cut-so-its-structural-evidence-is', { p0: native })
              : tx('trajectory:copy.message-groups-attempt-owned-records-with-no-logical-step-it-has-no-native-structural-reco')}
          </p>
        )}
      </div>
    </aside>
  );
}
