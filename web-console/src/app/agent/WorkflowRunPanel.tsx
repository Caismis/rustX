/* Copyright (c) 2026 DeepSeek. MIT. WorkflowRunPanel adapted; see PROVENANCE.md. */
import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import type { RuntimeClientInteractionHeader, WorkflowInstanceView, WorkflowNodeInstance, WorkflowRunId, WorkflowRunView, WorkflowState } from '../../../../protocol/app-server/v57';
import { useTranslation } from '../../locale/react';
import { DisclosureRow } from '../../presentation/primitives/DisclosureRow';
import { IconChevronRightOutline14 } from '../../presentation/primitives/icons';
import { StateDot, type StateDotState } from '../../presentation/primitives/StateDot';
import { useSubagents } from './subagent-context';
import css from '../../presentation/agent/WorkflowRunPanel.module.css';

export const workflowKey = (id: WorkflowRunId) => JSON.stringify([id.conversation_id, id.origin.type === 'tool' ? ['tool',id.origin.attempt_id] : ['scheduled',id.origin.start_id], id.invocation]);
const blockKey = (value: Pick<WorkflowInstanceView, 'block'>) => JSON.stringify([value.block.definition.workflow_id, value.block.definition.blocks, value.block.invocations]);
const nodeKey = (value: WorkflowInstanceView | WorkflowNodeInstance) => JSON.stringify([workflowKey(value.block.run), blockKey(value), value.node, value.visit]);
type Mode = 'clean' | 'running' | 'abnormal';
function mode(state: WorkflowState): Mode {
  return state.type !== 'settled' ? 'running' : state.outcome === 'completed' ? 'clean' : 'abnormal';
}
function dot(state: WorkflowState): StateDotState {
  return state.type !== 'settled' ? state.type === 'waiting' ? 'warning' : state.type === 'pending' ? 'idle' : 'ongoing'
    : state.outcome === 'completed' ? 'done' : state.outcome === 'failed' ? 'error' : 'warning';
}

/** DSH status-driven disclosure, retaining focus until an inspected body is left. */
function StatusDisclosure({ state, count, title, summary, children }: {
  state: WorkflowState; count: number; title: string; summary: ReactNode; children: ReactNode;
}) {
  const currentMode = mode(state), content = useRef<HTMLDivElement>(null);
  const [value, setValue] = useState(() => ({ mode: currentMode, count, open: currentMode !== 'clean', pending: false }));
  useLayoutEffect(() => {
    setValue(previous => {
      if (previous.mode === currentMode && previous.count === count) return previous;
      const focused = !!content.current?.contains(document.activeElement);
      const pending = currentMode === 'clean' && previous.open && focused;
      return { mode: currentMode, count, pending, open: currentMode === 'clean' ? pending
        : previous.mode === 'clean' || currentMode === 'abnormal' && previous.mode !== 'abnormal' ? true : previous.open };
    });
  }, [currentMode, count]);
  return <div onMouseDownCapture={value.pending ? event => {
    const header = event.currentTarget.querySelector('[data-disclosure-row]');
    if (header?.contains(event.target as Node)) event.preventDefault();
  } : undefined}><DisclosureRow icon={<IconChevronRightOutline14/>} title={title} open={value.open} expandable expandOnRowClick previewChevron={false} keepContentWhenOpen
    rowClassName={css.runHeader} leadingClassName={css.leading} titleClassName={css.title}
    onToggle={() => setValue(previous => ({ ...previous, open: !previous.open, pending: false }))} collapsedContent={summary}>
    <div ref={content} className={css.nodes} onBlur={event => {
      if (!event.currentTarget.contains(event.relatedTarget)) setValue(previous => previous.pending ? { ...previous, open: false, pending: false } : previous);
    }}>{children}</div>
  </DisclosureRow></div>;
}

/** One-shot member links keep their native activation and never acquire Agent controls. */
function WorkflowMember({ instance, children }: { instance: WorkflowInstanceView; children: ReactNode }) {
  const scope = useSubagents(), tx = useTranslation();
  const [focused, setFocused] = useState(false), [pending, setPending] = useState(false), [error, setError] = useState(false);
  const navigable = !!scope?.openWorkflow && instance.kind === 'agent' && !!instance.child && !!instance.child_agent_id && instance.state.type !== 'pending';
  const attributes = { 'data-workflow-node': instance.node!, 'data-workflow-activation-id': instance.child ?? undefined, 'data-node-state': instance.state.type };
  return <div className={css.memberHost}>
    {navigable || focused ? <button type="button" {...attributes} className={`${css.member} ${navigable ? css.memberButton : ''}`}
      aria-label={tx('common:subagents.open', { name: instance.node! })} aria-disabled={!navigable || pending || undefined} aria-busy={pending || undefined}
      tabIndex={navigable ? undefined : -1} onFocus={() => setFocused(true)} onBlur={() => setFocused(false)}
      onClick={() => {
        if (!navigable || pending) return;
        setPending(true); setError(false);
        void scope!.openWorkflow!(instance.child_agent_id!, instance.child!, instance.node!).catch(() => setError(true)).finally(() => setPending(false));
      }}>{children}</button>
      : <div {...attributes} className={css.member}>{children}</div>}
    {error && <span role="alert" className={css.empty}>{tx('common:activity.unavailable')}</span>}
  </div>;
}

/** Only admitted nodes, in native start order across blocks and loop iterations. */
export function WorkflowRunPanel({ run, pending = [] }: { run: WorkflowRunView; pending?: readonly RuntimeClientInteractionHeader[] }) {
  const tx = useTranslation();
  const label = (state: WorkflowState) => state.type === 'waiting' ? tx('common:activity.waiting-for', { state: tx(`common:state.${state.reason}`) })
      : tx(`common:state.${state.type === 'settled' ? state.outcome : state.type}`);
  const reviews = new Set(pending.flatMap(item => {
    if (item.kind !== 'review') return [];
    const node = item.workflow_node ?? (item.body?.kind.type === 'review' ? item.body.kind.review.instance : undefined);
    return node ? [nodeKey(node)] : [];
  }));
  const reviewing = (instance: WorkflowInstanceView) => run.state.type !== 'settled' && instance.state.type !== 'settled' && reviews.has(nodeKey(instance));
  const attention = run.instances.some(reviewing);
  const waiting = tx('common:activity.waiting-for', { state: tx('common:state.review') });
  const nodes = run.instances.filter(instance => instance.node != null);
  return <section className={css.root} data-workflow-run-id={workflowKey(run.id)} data-run-status={run.state.type} aria-label={tx('common:activity.workflow-label', { id: run.workflow_id })}>
    <StatusDisclosure state={run.state} count={nodes.length} title={run.workflow_id} summary={<>
      <span className={css.separator}/><span className={css.summary}>{tx('common:workflow.nodes', { count: nodes.length })}</span>
      <span className={css.statusTail}><StateDot state={attention ? 'warning' : dot(run.state)}/>{attention ? waiting : label(run.state)}</span>
    </>}>
      {nodes.map(instance => <WorkflowMember instance={instance} key={nodeKey(instance)}>
        <span className={css.dotSlot}><StateDot state={reviewing(instance) ? 'warning' : dot(instance.state)}/></span>
        <span className={css.memberCopy}><span className={css.memberLabel} title={instance.node!}>{instance.node}{instance.visit != null && instance.visit > 1 ? ` · ${instance.visit}` : ''}</span>
          {instance.kind !== 'agent' && <span className={css.kind}>{tx(`common:workflow.kind.${instance.kind}`)}
            {instance.iteration != null && <> · {tx('common:workflow.iteration', { current: instance.iteration, max: instance.iterations_max ?? '—' })}</>}
            {instance.loop_exit && <> · {tx(`common:workflow.loop.${instance.loop_exit}`)}</>}
            {instance.checks_passed != null && <> · {tx(instance.checks_passed ? 'common:workflow.checks-passed' : 'common:workflow.checks-failed')}</>}
            {instance.review_accepted != null && <> · {tx(instance.review_accepted ? 'common:workflow.accepted' : 'common:workflow.rejected')}</>}
          </span>}
          {instance.block.definition.blocks.length > 0 && <span className={css.kind}>
            {instance.block.definition.blocks.join(' / ')} · {instance.block.invocations.slice(1).join('.')}
          </span>}
        </span>
        <span className={css.memberStatus}>{reviewing(instance) ? waiting : label(instance.state)}</span>
      </WorkflowMember>)}
      {!nodes.length && <span className={css.empty}>{tx('common:workflow.empty')}</span>}
      {run.omitted_instances > 0 && <span className={css.empty}>{tx('common:activity.omitted', { n: run.omitted_instances })}</span>}
      <span className={css.budget}>{tx('common:activity.workflow-counts', { used: run.steps_consumed, max: run.steps_max, agents: run.agents_consumed, models: run.model_calls_consumed })}</span>
    </StatusDisclosure>
  </section>;
}
