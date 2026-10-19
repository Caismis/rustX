/* Copyright (c) 2026 DeepSeek. MIT. WorkflowRunPanel adapted; see PROVENANCE.md. */
import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import type { RuntimeClientInteractionHeader, WorkflowInstanceView, WorkflowNodeInstance, WorkflowRunId, WorkflowRunView, WorkflowState } from '../../../../protocol/app-server/v50';
import { useTranslation } from '../../locale/react';
import { DisclosureRow } from '../../presentation/primitives/DisclosureRow';
import { IconChevronRightOutline14 } from '../../presentation/primitives/icons';
import { StateDot, type StateDotState } from '../../presentation/primitives/StateDot';
import css from '../../presentation/agent/WorkflowRunPanel.module.css';

export const workflowKey = (id: WorkflowRunId) => JSON.stringify([id.conversation_id, id.attempt_id, id.invocation]);
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
function StatusDisclosure({ state, count, title, summary, root, children }: {
  state: WorkflowState; count: number; title: string; summary: ReactNode; root?: boolean; children: ReactNode;
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
    rowClassName={root ? css.runHeader : css.phaseHeader} leadingClassName={css.leading} titleClassName={css.title}
    onToggle={() => setValue(previous => ({ ...previous, open: !previous.open, pending: false }))} collapsedContent={summary}>
    <div ref={content} className={root ? css.phaseList : css.members} onBlur={event => {
      if (!event.currentTarget.contains(event.relatedTarget)) setValue(previous => previous.pending ? { ...previous, open: false, pending: false } : previous);
    }}>{children}</div>
  </DisclosureRow></div>;
}

/** Native block identities group nodes; budgets never masquerade as graph progress. */
export function WorkflowRunPanel({ run, pending = [] }: { run: WorkflowRunView; pending?: readonly RuntimeClientInteractionHeader[] }) {
  const tx = useTranslation();
  const label = (state: WorkflowState, unvisited = false) => unvisited ? tx('common:workflow.not-run')
    : state.type === 'waiting' ? tx('common:activity.waiting-for', { state: tx(`common:state.${state.reason}`) })
      : tx(`common:state.${state.type === 'settled' ? state.outcome : state.type}`);
  const reviews = new Set(pending.flatMap(item => {
    if (item.kind !== 'review') return [];
    const node = item.workflow_node ?? (item.body?.kind.type === 'review' ? item.body.kind.review.instance : undefined);
    return node ? [nodeKey(node)] : [];
  }));
  const reviewing = (instance: WorkflowInstanceView) => run.state.type !== 'settled' && instance.state.type !== 'settled' && reviews.has(nodeKey(instance));
  const attention = run.instances.some(reviewing);
  const waiting = tx('common:activity.waiting-for', { state: tx('common:state.review') });
  const groups = new Map<string, WorkflowInstanceView[]>();
  for (const instance of run.instances) {
    const key = blockKey(instance), list = groups.get(key) ?? [];
    list.push(instance); groups.set(key, list);
  }
  const nodes = run.instances.filter(instance => instance.node != null);
  return <section className={css.root} data-workflow-run-id={workflowKey(run.id)} data-run-status={run.state.type} aria-label={tx('common:activity.workflow-label', { id: run.workflow_id })}>
    <StatusDisclosure root state={run.state} count={nodes.length} title={run.workflow_id} summary={<>
      <span className={css.separator}/><span className={css.summary}>{tx('common:workflow.nodes', { count: nodes.length })}</span>
      <span className={css.statusTail}><StateDot state={attention ? 'warning' : dot(run.state)}/>{attention ? waiting : label(run.state)}</span>
    </>}>
      {[...groups].map(([key, instances]) => {
        const block = instances.find(instance => instance.kind === 'block'), first = instances[0], members = instances.filter(instance => instance.node != null);
        const state = block?.state ?? run.state;
        const review = members.some(reviewing);
        const name = first.block.definition.blocks.join(' / ') || tx('common:workflow.main');
        return <StatusDisclosure key={key} state={state} count={members.length} title={name}
          summary={<><span className={css.separator}/><span className={css.summary}>{tx('common:workflow.nodes', { count: members.length })}</span>{(block || review) && <span className={css.phaseStatus}>{review ? waiting : label(state)}</span>}</>}>
          {members.map(instance => <div className={css.member} key={JSON.stringify([instance.node, instance.visit])} data-workflow-node={instance.node} data-workflow-activation-id={instance.child ?? undefined} data-node-state={instance.state.type}>
            <span className={css.dotSlot}><StateDot state={reviewing(instance) ? 'warning' : dot(instance.state)}/></span>
            <span className={css.memberCopy}><span className={css.memberLabel} title={instance.node!}>{instance.node}{instance.visit != null && instance.visit > 1 ? ` · ${instance.visit}` : ''}</span>
              <span className={css.kind}>{tx(`common:workflow.kind.${instance.kind}`)}
                {instance.iteration != null && <> · {tx('common:workflow.iteration', { current: instance.iteration, max: instance.iterations_max ?? '—' })}</>}
                {instance.loop_exit && <> · {tx(`common:workflow.loop.${instance.loop_exit}`)}</>}
                {instance.checks_passed != null && <> · {tx(instance.checks_passed ? 'common:workflow.checks-passed' : 'common:workflow.checks-failed')}</>}
                {instance.review_accepted != null && <> · {tx(instance.review_accepted ? 'common:workflow.accepted' : 'common:workflow.rejected')}</>}
              </span>
            </span>
            <span className={css.memberStatus}>{reviewing(instance) ? waiting : label(instance.state, run.state.type === 'settled' && instance.state.type === 'pending')}</span>
          </div>)}
        </StatusDisclosure>;
      })}
      {!run.instances.length && <span className={css.empty}>{tx('common:workflow.empty')}</span>}
      {run.omitted_instances > 0 && <span className={css.empty}>{tx('common:activity.omitted', { n: run.omitted_instances })}</span>}
      <span className={css.budget}>{tx('common:activity.workflow-counts', { used: run.steps_consumed, max: run.steps_max, agents: run.agents_consumed })}</span>
    </StatusDisclosure>
  </section>;
}
