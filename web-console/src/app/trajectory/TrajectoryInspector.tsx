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
import { createContext, useContext, useEffect, useRef, useState, type ReactNode, type RefObject } from 'react';
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
} from '../../../../protocol/app-server/v20';
import { writeClipboard } from '../../presentation/primitives/clipboard';
import { Button } from '../../presentation/primitives/Button';
import { JsonTree, type JsonTreeLabels } from '../../presentation/primitives/JsonTree';
import { MarkdownText } from '../../presentation/markdown/MarkdownText';
import { CodeBlock } from '../../presentation/markdown/CodeBlock';
import { Tabs, TabList, Tab, TabPanel } from 'react-aria-components';
import { diffLines } from 'diff';
import type { TrajectoryFacet, TrajectorySelection } from './layout';
import { Artifact } from '../components/Artifact';
import { formatDuration, formatInstant } from './timeline';
import css from './Trajectory.module.css';
import { previewOf, cellLabel } from './TrajectoryCell';

const JSON_LABELS: JsonTreeLabels = {
  copyValue: 'Copy value',
  copyJson: 'Copy JSON',
  copyPath: 'Copy path',
  copyPrettyJson: 'Copy pretty JSON',
  copyCompactJson: 'Copy compact JSON',
  copied: 'Copied',
  copyFailed: 'Copy failed',
  collapseNode: 'Collapse',
  expandNode: 'Expand',
  copyButtonTitle: action => action,
};

/** A large integer the wire carries losslessly as a string. */
function count(value: string | number | null | undefined): number | undefined {
  if (value == null) return undefined;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

function Unavailable() {
  return <span className={css.unavailable}>Unavailable</span>;
}

function Truncated({ of }: { of: boolean | undefined }) {
  return of ? <p className={css.truncated}>Shown partially · truncated at the inspection bound</p> : null;
}

function Text({ value, markdown = false }: { value: TraceText; markdown?: boolean }) {
  const [copied, setCopied] = useState<string>();
  if (value.text === '' && !value.truncated) return <span className={css.unavailable}>Empty</span>;
  return (
    <>
      {markdown ? (
        <div className={css.markdown}>
          <MarkdownText text={value.text} />
        </div>
      ) : (
        <pre className={css.payload}>{value.text}</pre>
      )}
      <Button size="sm" className={css.copyText} onClick={() => { void writeClipboard(value.text).then(ok => setCopied(ok ? 'Copied' : 'Copy failed')); }}>{copied ?? 'Copy text'}</Button>
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
        labels={JSON_LABELS}
        collapsedStringLines={12}
        className={css.jsonTree}
        menuFocusOwner={body}
      />
      <Truncated of={value.truncated} />
    </>
  );
}

function Attachments({ artifacts }: { artifacts: readonly TraceArtifact[] }) {
  if (artifacts.length === 0) return <p className={css.unavailable}>No durable artifacts recorded</p>;
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
  switch (block.type) {
    case 'text':
      return <Text value={block.text} markdown />;
    case 'refusal':
      return (
        <div>
          <h4 className={css.blockLabel}>Refusal</h4>
          <Text value={block.text} markdown />
        </div>
      );
    case 'reasoning':
      return (
        <details className={css.reasoning}>
          <summary>Reasoning</summary>
          <Text value={block.text} markdown />
        </details>
      );
    case 'json':
      return <Structured value={block.value} label="Structured content" />;
    case 'tool_call':
      return (
        <div className={css.blockGroup}>
          <h4 className={css.blockLabel}>
            Proposed ToolCall · <span className={css.machine}>{block.name}</span>
          </h4>
          <dl className={css.facts}>
            <dt>ToolCall</dt>
            <dd className={css.machine}>{block.call_id}</dd>
            <dt>Tool</dt>
            <dd className={css.machine}>{block.tool_id}</dd>
          </dl>
          <Structured value={block.arguments} label={`${block.name} arguments`} />
        </div>
      );
    case 'tool_result':
      return (
        <div className={css.blockGroup}>
          <h4 className={css.blockLabel}>
            Tool result · <span className={css.machine}>{block.outcome}</span>
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
          Session upload · <span className={css.machine}>{block.name}</span>
        </p>
      );
  }
}

function Definition({ definition }: { definition: TraceToolDefinition }) {
  return (
    <div className={css.blockGroup}>
      <h4 className={css.blockLabel}>
        {definition.name} <span className={css.machine}>{definition.tool_id}</span>
      </h4>
      <Text value={definition.description} />
      <Structured value={definition.input_schema} label={`${definition.name} input schema`} />
    </div>
  );
}

/** Derived generation metrics, each unavailable unless its evidence exists. */
function Generation({ generation }: { generation: TraceGeneration }) {
  const ttft = count(generation.ttft_ms);
  const decode = count(generation.generation_ms);
  const rate = generation.output_tokens_per_second ?? undefined;
  return (
    <>
      <dt>Request duration (start → provider terminal)</dt>
      <dd>{formatDuration(count(generation.timeline?.terminal_ms))}</dd>
      <dt>Request start → dispatch</dt>
      <dd>{formatDuration(count(generation.timeline?.dispatch_ms))}</dd>
      <dt>Dispatch → first output (TTFT)</dt>
      <dd>{ttft === undefined ? <Unavailable /> : formatDuration(ttft)}</dd>
      <dt>First output → provider terminal</dt>
      <dd>{decode === undefined ? <Unavailable /> : formatDuration(decode)}</dd>
      <dt>Dispatch → provider terminal</dt>
      <dd>{formatDuration(count(generation.terminal_ms))}</dd>
      <dt>Throughput</dt>
      <dd>{rate === undefined ? <Unavailable /> : `${rate.toFixed(1)} tokens/s`}</dd>
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
const SYSTEM_PROMPT_STATE: Record<
  TraceSystemPromptPresentation['state'],
  { label: string; note: string }
> = {
  initial: {
    label: 'Initial',
    note: 'No earlier actual request exists in this conversation.',
  },
  changed: {
    label: 'Changed',
    note: 'The nearest preceding actual request froze a different prompt.',
  },
  unchanged: {
    label: 'Unchanged',
    note: 'The nearest preceding actual request froze the identical prompt.',
  },
  previous_unavailable: {
    label: 'Previous unavailable',
    note: 'A preceding request exists, but its frozen prompt could not be established at this read cut.',
  },
};

/** Display names for the closed Context presentation families. */
const CONTEXT_KIND: Record<TraceContextKind, string> = {
  native_environment: "Native context",
  goal_status: 'Goal status',
  runtime_tool_observation: 'Runtime tool observation',
  extension_environment: 'Extension environment',
  agent_status: 'Agent status',
};

/**
 * The exact native producer the server copied from the canonical message.
 *
 * Two certified extensions publish the same Context family, so the family
 * cannot name the producer and this renders the contributor identity the
 * server sent. No name is derived from the Context kind, and no extension
 * catalog is consulted: this is display of a resolved fact, not inference.
 */
function contextSource(source: TraceContextPresentation['source']): string {
  return source.type === 'runtime' ? 'Runtime' : `Extension ${source.contributor}`;
}

/**
 * The System Prompt relationship the server resolved for this request.
 *
 * Nothing here compares request details. The classification, and the page
 * independence that makes it trustworthy, are native facts; this renders
 * them and names the complete prompt's own home.
 */
function SystemPrompt({ system }: { system: TraceSystemPromptPresentation }) {
  const { label, note } = SYSTEM_PROMPT_STATE[system.state];
  return (
    <>
      <dt>System prompt</dt>
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
  return (
    <>
      <h3 className={css.sectionLabelHeading}>Context introduced by this request</h3>
      <p className={css.note}>
        The canonical context facts this actual request committed with its own start. A retry or
        recovery request reuses admitted context and introduces none.
      </p>
      {additions.length === 0 && (
        <p className={css.unavailable}>This request introduced no canonical context</p>
      )}
      {additions.map(addition => (
        <section key={addition.message_id} className={css.requestMessage} data-context-message-id={addition.message_id} data-selected={addition.message_id === selectedId || undefined}>
          <h4 className={css.blockLabel}>
            {CONTEXT_KIND[addition.context_kind]} · {contextSource(addition.source)}
            <span className={css.machine}> {addition.message_id}</span>
          </h4>
          {addition.preview ? <p>{addition.preview.text || "Empty"}</p> : <p>Content unavailable</p>}
          {addition.attachments.length > 0 && <Attachments artifacts={addition.attachments} />}
          <Truncated of={addition.truncated || addition.preview?.truncated} />
        </section>
      ))}
      {truncated && (
        <p className={css.truncated}>
          Further context facts omitted at the summary bound; the first, in native order, are shown.
        </p>
      )}
    </>
  );
}

/** The sections available for one record, given what the server projected. */
function sectionsOf(record: TraceRecord, detail: TraceDetail | undefined): TrajectoryFacet[] {
  if (record.kind === 'request') return ['Summary', 'System Prompt', 'Diff', 'Context', 'Tools', 'Options', 'Usage', 'Timing', 'Native'];
  if (record.kind === 'assistant') return ['Summary', 'Content', ...(detail?.messages.some(message => message.blocks.some(block => block.type === 'reasoning')) ? ['Thinking' as const] : []), 'Raw', 'Timing', 'Native'];
  if (record.kind === 'tool') return ['Summary', 'Input', ...(detail?.tool?.source ? ['Code' as const] : []), 'Result', 'Schema', 'Timing', 'Artifacts', 'Native'];
  return ['Summary', ...(detail?.messages.length ? ['Content' as const] : []), 'Timing', 'Artifacts', 'Native'];
}

/** Native classification is an input, never the output of jsdiff. */
function PromptDiff({ record, detail }: { record: TraceRecord; detail: TraceDetail | undefined }) {
  const request = detail?.request;
  if (!request) return <p className={css.unavailable}>Prompt content unavailable until detail is loaded.</p>;
  const previous = request.previous_system_prompt;
  if (request.predecessor.availability === 'not_applicable') return <p>No predecessor · initial prompt.</p>;
  if (request.predecessor.availability === 'unavailable' || !previous) return <p>Previous prompt unavailable · a complete diff cannot be produced.</p>;
  if (request.effective_system_prompt.truncated || previous.truncated) return <p className={css.truncated}>A complete diff cannot be produced: {request.effective_system_prompt.truncated ? 'current prompt truncated' : ''}{request.effective_system_prompt.truncated && previous.truncated ? '; ' : ''}{previous.truncated ? 'previous prompt truncated' : ''}. Native relationship: {record.request?.system_prompt.state}.</p>;
  if (record.request?.system_prompt.state === 'unchanged') return <p>No changes · complete frozen prompts are natively unchanged.</p>;
  if (record.request?.system_prompt.state !== 'changed') return <p>Native prompt relationship unavailable · a complete diff cannot be produced.</p>;
  const changes = diffLines(previous.text, request.effective_system_prompt.text, { maxEditLength: 4096 });
  if (!changes) return <p>A complete diff cannot be produced within the display work bound.</p>;
  return <pre className={css.diff} aria-label="System prompt diff">{changes.map((change, index) => <span key={index} data-change={change.added ? 'added' : change.removed ? 'removed' : 'context'}>{change.added ? '+ ' : change.removed ? '− ' : '  '}{change.value}</span>)}</pre>;
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
  const section = selection.facet;
  useEffect(() => {
    if (record.has_detail && !detail && !loading && !error) onLoadDetail(record.id);
  }, [record.id, record.has_detail, detail, loading, error, onLoadDetail]);
  const sections = sectionsOf(record, detail);
  const active = sections.includes(section) ? section : 'Summary';
  const request = detail?.request ?? undefined;
  const tool = detail?.tool ?? undefined;
  const messages = detail?.messages ?? [];
  const title =
    record.kind === 'request' && record.request
      ? `Request · ${record.request.model}`
      : record.kind === 'tool' && record.tool
        ? `Tool · ${record.tool.name ?? record.tool.tool_id}`
        : cellLabel[record.kind];

  return (
    <aside className={css.inspector} aria-label="Trace record inspector">
      <header>
        <strong>{title}</strong>
        <Button size="sm" onClick={onClose}>
          Close record
        </Button>
      </header>
      <Tabs className={css.inspectorTabs} selectedKey={active} onSelectionChange={key => onFacet(key as TrajectoryFacet)}>
      <TabList aria-label="Record sections" className={css.tabs}>
        {sections.map(name => <Tab key={name} id={name}>{name}</Tab>)}
      </TabList>
      {loading && (
        <p role="status" className={css.unavailable}>
          Loading record detail…
        </p>
      )}
      {error && (
        <p role="alert" className={css.error}>
          {error}
        </p>
      )}
      <InspectorPanel active={active} selectedId={selection.context_message_id} content={detail}>
        {active === 'Summary' && (
          <>
          <div className={css.summaryPreview}><MarkdownText text={previewOf(record)} /></div>
          {tool?.source && <section className={css.summaryPreview}>
            <h3 className={css.sectionLabelHeading}>Code</h3>
            <CodeBlock code={tool.source.text.text} lang={tool.source.language ?? undefined} lineNumbers copyLabel="Copy source" copiedLabel="Copied" />
            <Truncated of={tool.source.text.truncated} />
          </section>}
          {tool?.arguments && !tool.source && <section className={css.summaryPreview}>
            <h3 className={css.sectionLabelHeading}>Input</h3><Structured value={tool.arguments} label="Recorded arguments" />
          </section>}
          {tool?.result && <section className={css.summaryPreview}>
            <h3 className={css.sectionLabelHeading}>Result · {tool.result.outcome}</h3>
            {tool.result.blocks.map((block, index) => <Block key={index} block={block} />)}
            <Truncated of={tool.result.blocks_truncated} />
          </section>}
          <dl className={css.facts}>
            <dt>Status</dt><dd>{record.state}</dd>
            {record.request && <>
              <dt>Model</dt><dd>{record.request.model}</dd>
              <dt>Retry / recovery ordinal</dt><dd>{record.request.retry_number}</dd>
              <SystemPrompt system={record.request.system_prompt} />
              <dt>Tools</dt><dd>{record.request.tool_catalog.replaceAll('_', ' ')}</dd>
              <dt>Context introduced</dt><dd>{record.request.context_additions.length}{record.request.context_truncated ? ' · truncated' : ''}</dd>
              {record.request.failure_kind && <><dt>Failure</dt><dd>{record.request.failure_kind}</dd></>}
              <dt>Historical input</dt><dd><Button size="sm" onClick={() => onFacet('System Prompt')}>View System Prompt</Button> <Button size="sm" onClick={() => onFacet('Tools')}>View Tools</Button></dd>
              <dt>Acceptance</dt><dd>Provider completion alone does not prove canonical Assistant acceptance.</dd>
            </>}
            {record.calls.length > 0 && <><dt>Proposed calls</dt><dd>{record.calls.length} · A proposal proves assembly, not execution.</dd></>}
            {record.tool && <><dt>Execution</dt><dd>{record.tool.started ? 'Started · a durable start fact exists' : 'Proposed only'}</dd><dt>Outcome</dt><dd>{record.tool.outcome ?? 'Unknown'}</dd></>}
          </dl>
          <Truncated of={record.truncated || detail?.truncated} />

          </>
        )}

        {active === 'Native' && <>          <section className={css.nativeDetails}>
          <dl className={css.facts}>
            <dt>State</dt>
            <dd>{record.state}</dd>
            <dt>Record</dt>
            <dd className={css.machine}>{record.id}</dd>
            <dt>Attempt</dt>
            <dd className={css.machine}>{record.location.attempt_id ?? <Unavailable />}</dd>
            <dt>Logical Step</dt>
            <dd className={css.machine}>{record.location.step_id ?? <Unavailable />}</dd>
            {record.request && (
              <>
                <dt>Actual request</dt>
                <dd className={css.machine}>{record.request.request_id}</dd>
                <dt>Retry / recovery ordinal</dt>
                <dd>
                  {record.request.retry_number}
                  {record.request.retry_number > 0
                    ? ' · retry or recovery within this Step'
                    : ' · initial request'}
                </dd>
                <dt>Preceding request failure</dt>
                <dd>{record.request.previous_failure_kind ?? <Unavailable />}</dd>
                <dt>Failure class</dt>
                <dd>{record.request.failure_kind ?? 'None recorded'}</dd>
                <SystemPrompt system={record.request.system_prompt} />
                <dt>Context introduced</dt>
                <dd>
                  {record.request.context_additions.length}
                  {record.request.context_truncated ? ' (bounded)' : ''}
                </dd>
              </>
            )}
            {record.originating_tool_call_id && (
              <>
                <dt>Originating ToolCall</dt>
                <dd className={css.machine}>{record.originating_tool_call_id}</dd>
                <dd className={css.note}>
                  Server-resolved navigation correlation from this domain's own start fact. It
                  remains exact when the parent Tool row is outside the loaded window, and it
                  confers no lifecycle, ownership or settlement authority.
                </dd>
              </>
            )}
            {record.tool && (
              <>
                <dt>ToolCall</dt>
                <dd className={css.machine}>{record.tool.call_id}</dd>
                <dt>Tool</dt>
                <dd className={css.machine}>{record.tool.tool_id}</dd>
                <dt>Execution</dt>
                <dd>
                  {tool?.lifecycle === 'settled'
                    ? 'Settled · a canonical Tool message was accepted'
                    : record.tool.started
                      ? 'Started · a durable start fact exists'
                      : 'Proposed · assembly only, execution is not implied'}
                </dd>
                <dt>Outcome</dt>
                <dd>{record.tool.outcome ?? <Unavailable />}</dd>
                {record.tool.detail && (
                  <>
                    <dt>Detail</dt>
                    <dd>{record.tool.detail.text}</dd>
                  </>
                )}
              </>
            )}
            {record.calls.length > 0 && (
              <>
                <dt>Assembled ToolCalls</dt>
                <dd>
                  {record.calls.map(call => (
                    <div key={call.call_id} className={css.machine}>
                      {call.name} · {call.call_id}
                    </div>
                  ))}
                  <p className={css.note}>
                    A proposal proves assembly. Execution requires its own started record.
                  </p>
                </dd>
              </>
            )}
            {record.native_id && (
              <>
                <dt>Native identity</dt>
                <dd className={css.machine}>{record.native_id}</dd>
              </>
            )}
            {record.message_id && (
              <>
                <dt>Canonical message</dt>
                <dd className={css.machine}>{record.message_id}</dd>
              </>
            )}
            {record.kind === 'request' && (
              <>
                <dt>Acceptance</dt>
                <dd className={css.note}>
                  Provider completion alone does not prove canonical Assistant acceptance.
                </dd>
              </>
            )}
            <Truncated of={record.truncated} />
          </dl>
          </section></>}

        {active === 'Content' && messages.map(message => (
          <section key={message.message_id}>
            <dl className={css.facts}>
              <dt>Role</dt>
              <dd>{message.role}</dd>
              {message.source && (
                <>
                  <dt>Provenance</dt>
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
          <Structured value={{ value: messages, truncated: detail?.truncated ?? false }} label="Projected messages" />
        )}

        {active === 'System Prompt' && request && (<><h3 className={css.sectionLabelHeading}>Effective system prompt</h3><Text value={request.effective_system_prompt} markdown /></>)}

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
                <h3 className={css.sectionLabelHeading}>Accepted contribution</h3>
                <p className={css.note}>Frozen request context, not current domain state.</p>
                <Structured value={{ value: contribution, truncated: false }} label="Accepted contribution metadata" />
              </section>
            ))}
            <h3 className={css.sectionLabelHeading}>Reconstructed request context</h3>
            <p className={css.note}>
              The exact provider-neutral messages this request carried, rebuilt from its frozen
              snapshot and the historical Surface revision it referenced.
            </p>
            {request.messages.map((entry, index) => (
              <section key={entry.message_id ?? index} className={css.requestMessage} data-context-message-id={entry.message_id} data-selected={entry.message_id === selection.context_message_id || undefined}>
                <h4 className={css.blockLabel}>
                  {entry.role}
                  {entry.source ? ` · ${entry.source}` : ''}
                  {entry.message_id ? (
                    <span className={css.machine}> {entry.message_id}</span>
                  ) : (
                    <span className={css.note}> request-only, no canonical identity</span>
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
                Older context omitted at the inspection bound; the newest items are shown.
              </p>
            )}
          </>
        )}

        {active === 'Input' && tool && (
          <>
            <h3 className={css.sectionLabelHeading}>Recorded arguments</h3>
            {tool.arguments ? (
              <Structured value={tool.arguments} label={`${tool.name ?? tool.tool_id} arguments`} />
            ) : (
              <p className={css.unavailable}>
                The canonical proposal for this call is not loadable at this read cut.
              </p>
            )}
          </>
        )}

        {active === 'Code' && tool?.source && (
          <>
            <h3 className={css.sectionLabelHeading}>
              Program source · <span className={css.machine}>{tool.source.field}</span>
            </h3>
            <p className={css.note}>
              {tool.source.language
                ? `Highlighted as ${tool.source.language}; the native Tool contract fixes this language.`
                : 'No language is highlighted: this Tool contract identifies the field as source without fixing a language.'}
            </p>
            <CodeBlock
              code={tool.source.text.text}
              lang={tool.source.language ?? undefined}
              lineNumbers
              copyLabel="Copy source"
              copiedLabel="Copied"
            />
            <Truncated of={tool.source.text.truncated} />
          </>
        )}

        {active === 'Result' && tool?.result && (
          <>
            <dl className={css.facts}>
              <dt>Outcome</dt>
              <dd>{tool.result.outcome}</dd>
              <dt>Execution duration</dt>
              <dd>{formatDuration(count(tool.result.duration_ms))}</dd>
              {tool.result.exit_code != null && (
                <>
                  <dt>Exit code</dt>
                  <dd className={css.machine}>{tool.result.exit_code}</dd>
                </>
              )}
              {tool.result.truncation && (
                <>
                  <dt>Tool-recorded truncation</dt>
                  <dd>
                    {tool.result.truncation.truncated ? 'Output was truncated by the Tool' : 'Complete'}
                    {tool.result.truncation.original_bytes != null
                      ? ` · ${tool.result.truncation.original_bytes} bytes originally`
                      : ''}
                  </dd>
                </>
              )}
              {tool.result.managed_output && (
                <>
                  <dt>Managed output</dt>
                  <dd>
                    {tool.result.managed_output.available
                      ? tool.result.managed_output.complete
                        ? 'Complete'
                        : 'Partial'
                      : 'Unavailable'}
                  </dd>
                  {tool.result.managed_output.locator != null ? (
                    <>
                      <dt>Locator</dt>
                      <dd>{tool.result.managed_output.locator}</dd>
                    </>
                  ) : null}
                  {tool.result.managed_output.diagnostic ? (
                    <>
                      <dt>Diagnostic</dt>
                      <dd><Text value={tool.result.managed_output.diagnostic} /></dd>
                    </>
                  ) : null}
                </>
              )}
            </dl>
            {tool.result.detail && (
              <>
                <h3 className={css.sectionLabelHeading}>Status detail</h3>
                <Text value={tool.result.detail} />
              </>
            )}
            {tool.result.blocks.map((block, index) => (
              <Block key={index} block={block} />
            ))}
            <Truncated of={tool.result.blocks_truncated} />
          </>
        )}

        {active === 'Schema' && tool?.definition && <Definition definition={tool.definition} />}

        {active === 'Tools' && request && (
          <>
            <p className={css.note}>
              The exact historical Tool catalog this request carried, not the current one.
            </p>
            {request.tools.length === 0 && <p className={css.unavailable}>No Tool definitions recorded</p>}
            {request.tools.map(definition => (
              <Definition key={definition.tool_id} definition={definition} />
            ))}
            {request.tools_truncated && (
              <p className={css.truncated}>Further Tool definitions omitted at the inspection bound</p>
            )}
          </>
        )}

        {active === 'Options' && request && (
          <>
            <dl className={css.facts}>
              <dt>Model</dt>
              <dd className={css.machine}>{request.model}</dd>
              <dt>Protocol</dt>
              <dd className={css.machine}>{request.protocol}</dd>
              <dt>Output token limit</dt>
              <dd className={css.machine}>{request.max_output_tokens}</dd>
              <dt>Context window</dt>
              <dd className={css.machine}>{request.context_window_tokens}</dd>
              <dt>Reasoning</dt>
              <dd>
                {request.reasoning_enabled ? 'enabled' : 'disabled'}
                {request.reasoning_profile ? ` · ${request.reasoning_profile}` : ''}
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
                {request.omitted_option_count} configured request parameter
                {request.omitted_option_count === 1 ? '' : 's'} outside the inspection allowlist
                {request.omitted_option_count === 1 ? ' is' : ' are'} not shown.
              </p>
            )}
          </>
        )}

        {active === 'Usage' && (
          <dl className={css.facts}>
            {record.request?.usage ? (
              <>
                <dt>Input tokens</dt>
                <dd className={css.machine}>{record.request.usage.input_tokens}</dd>
                <dt>Output tokens</dt>
                <dd className={css.machine}>{record.request.usage.output_tokens}</dd>
                <dt>Total tokens</dt>
                <dd className={css.machine}>{record.request.usage.total_tokens}</dd>
                <dt>Reasoning tokens</dt>
                <dd className={css.machine}>
                  {record.request.usage.details?.reasoning_tokens ?? <Unavailable />}
                </dd>
                <dt>Cached input</dt>
                <dd className={css.machine}>
                  {record.request.usage.details?.cached_input_tokens ?? <Unavailable />}
                </dd>
              </>
            ) : (
              <>
                <dt>Usage</dt>
                <dd>
                  <Unavailable />
                </dd>
              </>
            )}
          </dl>
        )}

        {active === 'Timing' && (
          <dl className={css.facts}>
            <dt>Started</dt>
            <dd className={css.machine}>{formatInstant(record.timing.started_at)}</dd>
            <dt>Ended</dt>
            <dd className={css.machine}>{formatInstant(record.timing.ended_at)}</dd>
            <dt>{record.kind === 'request' ? 'Journal wall duration' : 'Duration'}</dt>
            <dd>{record.timing.duration_ms == null ? <Unavailable /> : formatDuration(count(record.timing.duration_ms))}</dd>
            {record.request?.generation ? (
              <Generation generation={record.request.generation} />
            ) : record.kind === 'request' ? (
              <>
                <dt>Generation evidence</dt>
                <dd className={css.note}>
                  No settled generation evidence was recorded for this request.
                </dd>
              </>
            ) : null}
            <dt>Source</dt>
            <dd className={css.note}>
              {record.timing.duration_ms == null
                ? 'One endpoint only; an in-flight or unterminated record has no duration.'
                : 'Two authoritative durable timestamps.'}
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
