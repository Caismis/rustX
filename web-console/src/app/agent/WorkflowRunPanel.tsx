/* Copyright (c) 2026 DeepSeek. MIT. WorkflowRunPanel adapted; see PROVENANCE.md. */
import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import type { RuntimeClientInteractionHeader, WorkflowInstanceView, WorkflowNodeInstance, WorkflowRunView, WorkflowState } from '../../../../protocol/app-server/v78';
import { useTranslation } from '../../locale/react';
import { DisclosureRow } from '../../presentation/primitives/DisclosureRow';
import { IconChevronRightOutline14 } from '../../presentation/primitives/icons';
import { StateDot } from '../../presentation/primitives/StateDot';
import { KindIcon } from '../workflows/ui/icons';
import { useSubagents } from './subagent-context';
import { workflowChildren, workflowKey } from '../../bindings/workflow-children';
import { useWorkflowRunCanvas } from '../workflows/run-canvas/context';
import { workflowDot as dot, workflowStatus } from './workflow-state';
import css from '../../presentation/agent/WorkflowRunPanel.module.css';

const blockKey = (value: Pick<WorkflowInstanceView, 'block'>) => JSON.stringify([value.block.definition.workflow_id, value.block.definition.blocks, value.block.invocations]);
const nodeKey = (value: WorkflowInstanceView | WorkflowNodeInstance) => JSON.stringify([workflowKey(value.block.run), blockKey(value), value.node, value.visit]);
type Mode = 'clean' | 'running' | 'abnormal';
function mode(state: WorkflowState): Mode {
  return state.type !== 'settled' ? 'running' : state.outcome === 'completed' ? 'clean' : 'abnormal';
}

/** DSH status-driven disclosure, retaining focus until an inspected body is left. */
function StatusDisclosure({ state, count, title, summary, headerAction, children }: {
  state: WorkflowState; count: number; title: string; summary: ReactNode; headerAction?: ReactNode; children: ReactNode;
}) {
  const currentMode = mode(state), content = useRef<HTMLDivElement>(null);
  const [value, setValue] = useState(() => ({ mode: currentMode, count, open: currentMode !== 'clean', pending: false }));
  useLayoutEffect(() => {
    setValue(previous => {
      if (previous.mode === currentMode) return previous.count === count ? previous : { ...previous, count };
      const focused = !!content.current?.contains(document.activeElement);
      const pending = currentMode === 'clean' && previous.open && focused;
      return { mode: currentMode, count, pending, open: currentMode === 'clean' ? pending
        : previous.mode === 'clean' || currentMode === 'abnormal' && previous.mode !== 'abnormal' ? true : previous.open };
    });
  }, [currentMode, count]);
  const disclosure = <div className={headerAction ? css.headerMain : undefined} onMouseDownCapture={value.pending ? event => {
    const header = event.currentTarget.querySelector('[data-disclosure-row]');
    if (header?.contains(event.target as Node)) event.preventDefault();
  } : undefined}><DisclosureRow icon={<IconChevronRightOutline14/>} title={title} open={value.open} expandable expandOnRowClick previewChevron={false} keepContentWhenOpen
    rowClassName={css.runHeader} leadingClassName={css.leading} titleClassName={css.title}
    contentClassName={css.headerContent} contentLayoutClassName={css.headerContentLayout}
    onToggle={() => setValue(previous => ({ ...previous, open: !previous.open, pending: false }))} collapsedContent={summary}>
    <div ref={content} className={css.nodes} onBlur={event => {
      if (!event.currentTarget.contains(event.relatedTarget)) setValue(previous => previous.pending ? { ...previous, open: false, pending: false } : previous);
    }}>{children}</div>
  </DisclosureRow></div>;
  return headerAction ? <div className={css.headerRow}>{disclosure}{headerAction}</div> : disclosure;
}

/** One-shot member links keep their native activation and never acquire Agent controls. */
function WorkflowMember({ instance, onInspectCanvas, children }: {
  instance: WorkflowInstanceView;
  onInspectCanvas?: (trigger: HTMLElement | null) => void;
  children: ReactNode;
}) {
  const scope = useSubagents(), tx = useTranslation();
  const [focused, setFocused] = useState(false), [pending, setPending] = useState(false), [error, setError] = useState(false);
  const navigable = !!scope?.openWorkflow && instance.kind === 'agent' && !!instance.child && !!instance.child_agent_id && instance.state.type !== 'pending';
  const canvasOnly = !navigable && !!onInspectCanvas;
  const attributes = { 'data-workflow-node': instance.node!, 'data-workflow-activation-id': instance.child ?? undefined, 'data-node-state': instance.state.type };
  return <div className={css.memberHost}>
    {navigable || canvasOnly || focused ? <button type="button" {...attributes} className={`${css.member} ${navigable || canvasOnly ? css.memberButton : ''}`}
      aria-label={tx(canvasOnly ? 'workflows:run-canvas.inspect-node' : 'common:subagents.open', canvasOnly ? { p0: instance.node! } : { name: instance.node! })}
      aria-disabled={(!navigable && !canvasOnly) || pending || undefined} aria-busy={pending || undefined}
      tabIndex={navigable || canvasOnly ? undefined : -1} onFocus={() => setFocused(true)} onBlur={() => setFocused(false)}
      onClick={event => {
        if (canvasOnly) { onInspectCanvas(event.currentTarget); return; }
        if (!navigable || pending) return;
        setPending(true); setError(false);
        void scope!.openWorkflow!(instance.child_agent_id!, instance.child!, instance.node!).catch(() => setError(true)).finally(() => setPending(false));
      }}>{children}</button>
      : <div {...attributes} className={css.member}>{children}</div>}
    {navigable && onInspectCanvas && <button type="button" className={css.memberCanvasButton}
      aria-label={tx('workflows:run-canvas.inspect-node', { p0: instance.node! })}
      title={tx('workflows:run-canvas.inspect-node', { p0: instance.node! })}
      onClick={event => { event.stopPropagation(); onInspectCanvas(event.currentTarget); }}>↗</button>}
    {error && <span role="alert" className={css.empty}>{tx('common:activity.unavailable')}</span>}
  </div>;
}

/** Only admitted nodes, in native start order across blocks and loop iterations. */
export function WorkflowRunPanel({ run, pending = [] }: { run: WorkflowRunView; pending?: readonly RuntimeClientInteractionHeader[] }) {
  const tx = useTranslation();
  const runCanvas = useWorkflowRunCanvas();
  const label = (state: WorkflowState) => workflowStatus(state, tx);
  const reviews = new Set(pending.flatMap(item => {
    if (item.kind !== 'review') return [];
    const node = item.workflow_node ?? (item.body?.kind.type === 'review' ? item.body.kind.review.instance : undefined);
    return node ? [nodeKey(node)] : [];
  }));
  const reviewing = (instance: WorkflowInstanceView) => run.state.type !== 'settled' && instance.state.type !== 'settled' && reviews.has(nodeKey(instance));
  const attention = run.instances.some(reviewing);
  const waiting = tx('common:activity.waiting-for', { state: tx('common:state.review') });
  const nodes = run.instances.filter(instance => instance.node != null);
  const children = workflowChildren([run], run.id.conversation_id);
  const openRun = (selected?: WorkflowInstanceView, trigger?: HTMLElement | null) => {
    runCanvas?.openRunCanvas({
      runId: run.id,
      workflowId: run.workflow_id,
      conversationId: run.id.conversation_id,
      selectedVisit: selected?.node != null && selected.visit != null
        ? { block: selected.block, node: selected.node, visit: selected.visit }
        : undefined,
      sourceMode: 'live',
    }, trigger);
  };
  return <section className={css.root} data-workflow-run-id={workflowKey(run.id)} data-run-status={run.state.type} aria-label={tx('common:activity.workflow-label', { id: run.workflow_id })}>
    <StatusDisclosure state={run.state} count={nodes.length} title={run.workflow_id}
      headerAction={runCanvas && <button type="button" className={css.canvasHeaderButton}
        aria-label={tx('workflows:run-canvas.open')} title={tx('workflows:run-canvas.open')}
        onClick={event => openRun(undefined, event.currentTarget)}>
        {tx('workflows:run-canvas.open')}
      </button>}
      summary={<>
      <span className={css.separator} aria-hidden="true"/><span className={`${css.summary} ${children.length ? css.withChildren : ''}`}>
        <span className={css.nodeCount}>{tx('common:workflow.nodes', { count: nodes.length })}{children.length > 0 && ' · '}</span>
        {children.length > 0 && tx(run.omitted_instances > 0 ? 'common:subagents.count-at-least' : 'common:subagents.count', { count: children.length })}
      </span>
      <span className={css.statusTail}><StateDot state={attention ? 'warning' : dot(run.state)}/>{attention ? waiting : label(run.state)}</span>
    </>}>
      {nodes.map(instance => <WorkflowMember instance={instance} key={nodeKey(instance)}
        onInspectCanvas={runCanvas ? trigger => openRun(instance, trigger) : undefined}>
        <span className={css.nodeIcon}><KindIcon kind={instance.kind === 'block' ? 'start' : instance.kind} size={14}/></span>
        <span className={css.memberCopy}><span className={css.memberLabel} title={instance.node!}>{instance.node}{instance.visit != null && instance.visit > 1 ? ` · ${instance.visit}` : ''}</span>
          {instance.kind === 'agent' ? <span className={css.kind}>{tx('common:workflow.agent-record')}</span> : <span className={css.kind}>{tx(`common:workflow.kind.${instance.kind}`)}
            {instance.iteration != null && <> · {tx('common:workflow.iteration', { current: instance.iteration, max: instance.iterations_max ?? '—' })}</>}
            {instance.loop_exit && <> · {tx(`common:workflow.loop.${instance.loop_exit}`)}</>}
            {instance.checks_passed != null && <> · {tx(instance.checks_passed ? 'common:workflow.checks-passed' : 'common:workflow.checks-failed')}</>}
          </span>}
          {instance.block.definition.blocks.length > 0 && <span className={css.kind}>
            {instance.block.definition.blocks.join(' / ')} · {instance.block.invocations.slice(1).join('.')}
          </span>}
        </span>
        <span className={css.memberStatus}><StateDot state={reviewing(instance) ? 'warning' : dot(instance.state)}/>{reviewing(instance) ? waiting : label(instance.state)}</span>
      </WorkflowMember>)}
      {!nodes.length && <span className={css.empty}>{tx('common:workflow.empty')}</span>}
      {run.omitted_instances > 0 && <span className={css.empty}>{tx('common:activity.omitted', { n: run.omitted_instances })}</span>}
    </StatusDisclosure>
  </section>;
}
