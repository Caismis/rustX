/* Copyright (c) 2026 DeepSeek. MIT. WorkflowRunPanel adapted; see PROVENANCE.md. */
import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import type { RuntimeClientInteractionHeader, WorkflowInstanceView, WorkflowNodeInstance, WorkflowRunView, WorkflowState } from '../../../../protocol/app-server/v80';
import { useTranslation } from '../../locale/react';
import { DisclosureRow } from '../../presentation/primitives/DisclosureRow';
import { IconChevronRightOutline14 } from '../../presentation/primitives/icons';
import { StateDot } from '../../presentation/primitives/StateDot';
import { KindIcon } from '../workflows/ui/icons';
import { useSubagents } from './subagent-context';
import { workflowChildren, workflowKey } from '../../bindings/workflow-children';
import { useWorkflowRunCanvas } from '../workflows/run-canvas/context';
import { workflowDot as dot, workflowStatus } from './workflow-state';

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
  const disclosure = <div className={headerAction ? "flex-1 min-w-0" : undefined} onMouseDownCapture={value.pending ? event => {
    const header = event.currentTarget.querySelector('[data-disclosure-row]');
    if (header?.contains(event.target as Node)) event.preventDefault();
  } : undefined}><DisclosureRow icon={<IconChevronRightOutline14/>} title={title} open={value.open} expandable expandOnRowClick previewChevron={false} keepContentWhenOpen
    rowClassName="box-border flex items-center gap-[6px] w-full min-w-0 h-[32px] px-[8px] py-0 rounded-[var(--dsw-radius-md)] bg-[var(--dsw-alias-bg-module-platform)] focus-visible:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] focus-visible:-outline-offset-2 focus-visible:rounded-[var(--dsw-radius-md)]" leadingClassName="mr-0 [color:inherit]" titleClassName="overflow-hidden flex-none min-w-0 max-w-[42%] max-[560px]:max-w-[34%] [color:inherit] font-[510] text-ellipsis whitespace-nowrap"
    contentClassName="flex-1" contentLayoutClassName="gap-[6px]"
    onToggle={() => setValue(previous => ({ ...previous, open: !previous.open, pending: false }))} collapsedContent={summary}>
    <div ref={content} className="flex flex-col gap-[2px] min-w-0 pt-[4px] pr-0 pb-0 pl-[16px] max-[560px]:pl-[12px]" onBlur={event => {
      if (!event.currentTarget.contains(event.relatedTarget)) setValue(previous => previous.pending ? { ...previous, open: false, pending: false } : previous);
    }}>{children}</div>
  </DisclosureRow></div>;
  return headerAction ? <div className="flex items-start gap-[6px] min-w-0 [&>button]:mt-[4px]">{disclosure}{headerAction}</div> : disclosure;
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
  return <div className="flex items-center gap-[6px] min-w-0">
    {navigable || canvasOnly || focused ? <button type="button" {...attributes} className={`flex items-center gap-[12px] max-[560px]:gap-[6px] w-full min-w-0 min-h-[calc(24px+var(--dsh-content-font-delta,0px))] [color:var(--dsw-alias-label-secondary)] box-border p-0 border-0 rounded-[var(--dsw-radius-sm)] bg-transparent [font:inherit] text-left ${navigable || canvasOnly ? 'cursor-pointer focus-visible:outline-none [&_.member-label]:[color:var(--dsw-alias-link)] [&_.member-label]:font-medium hover:[&_.member-label]:underline-dotted hover:[&_.member-label]:[text-underline-offset:3px] focus-visible:[&_.member-label]:underline-dotted focus-visible:[&_.member-label]:[text-underline-offset:3px] focus-visible:[&_.member-copy]:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color)] focus-visible:[&_.member-copy]:-outline-offset-1 focus-visible:[&_.member-copy]:rounded-[var(--dsw-radius-sm)]' : ''}`}
      aria-label={tx(canvasOnly ? 'workflows:run-canvas.inspect-node' : 'common:subagents.open', canvasOnly ? { p0: instance.node! } : { name: instance.node! })}
      aria-disabled={(!navigable && !canvasOnly) || pending || undefined} aria-busy={pending || undefined}
      tabIndex={navigable || canvasOnly ? undefined : -1} onFocus={() => setFocused(true)} onBlur={() => setFocused(false)}
      onClick={event => {
        if (canvasOnly) { onInspectCanvas(event.currentTarget); return; }
        if (!navigable || pending) return;
        setPending(true); setError(false);
        void scope!.openWorkflow!(instance.child_agent_id!, instance.child!, instance.node!).catch(() => setError(true)).finally(() => setPending(false));
      }}>{children}</button>
      : <div {...attributes} className="flex items-center gap-[12px] max-[560px]:gap-[6px] w-full min-w-0 min-h-[calc(24px+var(--dsh-content-font-delta,0px))] [color:var(--dsw-alias-label-secondary)] box-border p-0 border-0 rounded-[var(--dsw-radius-sm)] bg-transparent [font:inherit] text-left">{children}</div>}
    {navigable && onInspectCanvas && <button type="button" className="flex-none inline-flex items-center justify-center h-[24px] px-[8px] py-0 [border:1px_solid_var(--dsw-alias-border-l3)] rounded-[var(--dsw-radius-sm)] bg-[var(--dsw-alias-bg-layer-2)] [color:var(--dsw-alias-label-secondary)] [font:inherit] text-[11px] font-medium cursor-pointer hover:bg-[var(--dsw-alias-interactive-bg-hover)] hover:[color:var(--dsw-alias-label-primary)] focus-visible:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color)] focus-visible:outline-offset-1"
      aria-label={tx('workflows:run-canvas.inspect-node', { p0: instance.node! })}
      title={tx('workflows:run-canvas.inspect-node', { p0: instance.node! })}
      onClick={event => { event.stopPropagation(); onInspectCanvas(event.currentTarget); }}>↗</button>}
    {error && <span role="alert" className="[color:var(--dsw-alias-label-tertiary)] text-[var(--dsh-content-font-size-secondary,13px)] leading-[calc(20px+var(--dsh-content-font-delta-secondary,0px))] wrap-anywhere">{tx('common:activity.unavailable')}</span>}
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
  return <section className="w-full min-w-0 [color:var(--dsw-alias-label-secondary)] [&+&]:mt-[4px]" data-workflow-run-id={workflowKey(run.id)} data-run-status={run.state.type} aria-label={tx('common:activity.workflow-label', { id: run.workflow_id })}>
    <StatusDisclosure state={run.state} count={nodes.length} title={run.workflow_id}
      headerAction={runCanvas && <button type="button" className="flex-none inline-flex items-center justify-center h-[24px] px-[8px] py-0 [border:1px_solid_var(--dsw-alias-border-l3)] rounded-[var(--dsw-radius-sm)] bg-[var(--dsw-alias-bg-layer-2)] [color:var(--dsw-alias-label-secondary)] [font:inherit] text-[11px] font-medium cursor-pointer hover:bg-[var(--dsw-alias-interactive-bg-hover)] hover:[color:var(--dsw-alias-label-primary)] focus-visible:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color)] focus-visible:outline-offset-1"
        aria-label={tx('workflows:run-canvas.open')} title={tx('workflows:run-canvas.open')}
        onClick={event => openRun(undefined, event.currentTarget)}>
        {tx('workflows:run-canvas.open')}
      </button>}
      summary={<>
      <span className="flex-none w-[2px] h-[2px] rounded-full bg-[var(--dsw-alias-label-tertiary)]" aria-hidden="true"/><span className={`overflow-hidden flex-1 min-w-0 text-[var(--dsh-content-font-size-secondary,13px)] text-ellipsis whitespace-nowrap `}>
        <span className={children.length ? "max-[560px]:hidden" : undefined}>{tx('common:workflow.nodes', { count: nodes.length })}{children.length > 0 && ' · '}</span>
        {children.length > 0 && tx(run.omitted_instances > 0 ? 'common:subagents.count-at-least' : 'common:subagents.count', { count: children.length })}
      </span>
      <span className="inline-flex flex-none items-center gap-[4px] max-w-[45%] text-[11px] font-[510] whitespace-nowrap overflow-hidden"><StateDot state={attention ? 'warning' : dot(run.state)}/>{attention ? waiting : label(run.state)}</span>
    </>}>
      {nodes.map(instance => <WorkflowMember instance={instance} key={nodeKey(instance)}
        onInspectCanvas={runCanvas ? trigger => openRun(instance, trigger) : undefined}>
        <span className="inline-flex flex-none w-[20px] h-[20px] items-center justify-center rounded-[5px] [color:var(--dsw-alias-state-business-primary)] bg-[var(--dsw-alias-interactive-bg-hover)]"><KindIcon kind={instance.kind === 'block' ? 'start' : instance.kind} size={14}/></span>
        <span className="member-copy flex flex-col flex-1 min-w-0 px-0 py-[3px]"><span className="member-label overflow-hidden text-[var(--dsh-content-font-size,14px)] leading-[calc(24px+var(--dsh-content-font-delta,0px))] text-ellipsis whitespace-nowrap" title={instance.node!}>{instance.node}{instance.visit != null && instance.visit > 1 ? ` · ${instance.visit}` : ''}</span>
          {instance.kind === 'agent' ? <span className="[color:var(--dsw-alias-label-tertiary)] text-[var(--dsh-content-font-size-secondary,13px)] leading-[calc(20px+var(--dsh-content-font-delta-secondary,0px))] wrap-anywhere">{tx('common:workflow.agent-record')}</span> : <span className="[color:var(--dsw-alias-label-tertiary)] text-[var(--dsh-content-font-size-secondary,13px)] leading-[calc(20px+var(--dsh-content-font-delta-secondary,0px))] wrap-anywhere">{tx(`common:workflow.kind.${instance.kind}`)}
            {instance.iteration != null && <> · {tx('common:workflow.iteration', { current: instance.iteration, max: instance.iterations_max ?? '—' })}</>}
            {instance.loop_exit && <> · {tx(`common:workflow.loop.${instance.loop_exit}`)}</>}
            {instance.checks_passed != null && <> · {tx(instance.checks_passed ? 'common:workflow.checks-passed' : 'common:workflow.checks-failed')}</>}
          </span>}
          {instance.block.definition.blocks.length > 0 && <span className="[color:var(--dsw-alias-label-tertiary)] text-[var(--dsh-content-font-size-secondary,13px)] leading-[calc(20px+var(--dsh-content-font-delta-secondary,0px))] wrap-anywhere">
            {instance.block.definition.blocks.join(' / ')} · {instance.block.invocations.slice(1).join('.')}
          </span>}
        </span>
        <span className="inline-flex flex-none items-center justify-end gap-[4px] max-w-[42%] overflow-hidden text-[var(--dsh-content-font-size-secondary,13px)] text-right text-ellipsis whitespace-nowrap"><StateDot state={reviewing(instance) ? 'warning' : dot(instance.state)}/>{reviewing(instance) ? waiting : label(instance.state)}</span>
      </WorkflowMember>)}
      {!nodes.length && <span className="[color:var(--dsw-alias-label-tertiary)] text-[var(--dsh-content-font-size-secondary,13px)] leading-[calc(20px+var(--dsh-content-font-delta-secondary,0px))] wrap-anywhere">{tx('common:workflow.empty')}</span>}
      {run.omitted_instances > 0 && <span className="[color:var(--dsw-alias-label-tertiary)] text-[var(--dsh-content-font-size-secondary,13px)] leading-[calc(20px+var(--dsh-content-font-delta-secondary,0px))] wrap-anywhere">{tx('common:activity.omitted', { n: run.omitted_instances })}</span>}
    </StatusDisclosure>
  </section>;
}
