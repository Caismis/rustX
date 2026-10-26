import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { WorkflowRunPanel } from '../src/app/agent/WorkflowRunPanel';
import { SubagentContext } from '../src/app/agent/subagent-context';
import type { AppServerClient } from '../src/client/app-server';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { Review } from '../src/app/agent/Review';
import { workflowReview, workflowRun } from './workflow-data';
import { interaction, snapshot } from './fixture';
import { nativeTool } from './native-tool-cases';
afterEach(cleanup);

it('shows the admitted path while waiting, labels the budget correctly, and collapses only on native settlement', () => {
  const run = workflowRun(), ui = render(<WorkflowRunPanel run={run}/>);
  for (const name of ['read', 'triage', 'analyze', 'gate']) expect(ui.getByText(name)).toBeTruthy();
  expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(4);
  expect(ui.container.textContent).toContain('Steps used 9 · Limit 10 · Agent calls 2 · Model calls 0');
  expect(ui.container.textContent).not.toContain('9 / 10');
  run.steps_consumed = 10; ui.rerender(<WorkflowRunPanel run={run}/>);
  expect(ui.container.querySelector('[data-run-status]')?.getAttribute('data-run-status')).toBe('waiting');
  run.state = { type: 'settled', outcome: 'completed' }; run.instances[0].state = run.state;
  ui.rerender(<WorkflowRunPanel run={run}/>);
  expect(ui.getByRole('button').getAttribute('aria-expanded')).toBe('false');
  fireEvent.click(ui.getByRole('button'));
  expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(4);
  expect(ui.queryByText('Not run')).toBeNull();
  expect(ui.container.querySelector('pre')).toBeNull();
});
it('keeps abnormal and loop outcomes visible and does not create Agent controls for finite activations', () => {
  const run = workflowRun(); run.state = { type: 'settled', outcome: 'failed' };
  run.instances[4].state = { type: 'settled', outcome: 'denied' };
  run.instances.push({ ...run.instances[4], node: 'loop', kind: 'loop', iteration: 3, iterations_max: 3, loop_exit: 'exhausted', checks_passed: false });
  run.instances[1].child = 'finite-activation';
  const ui = render(<WorkflowRunPanel run={run}/>);
  expect(ui.getByText(/Iteration limit reached/)).toBeTruthy();
  expect(ui.getByText(/Checks failed/)).toBeTruthy();
  expect(ui.container.querySelector('[data-workflow-activation-id="finite-activation"] button')).toBeNull();
});
it('keeps interleaved branches and repeated loop nodes in native start order across updates', () => {
  const run = workflowRun(), root = run.instances[0].block;
  const branch = (name: string, iteration: number) => ({ ...root,
    definition: { ...root.definition, blocks: ['fork', name, 'repeat', 'body'] }, invocations: [0, 0, iteration] });
  const node = (name: string, block = root) => ({ ...run.instances[1], node: name, block });
  run.instances = [run.instances[0], node('fork'), node('read', branch('zeta', 1)), node('read', branch('alpha', 1)), node('read', branch('zeta', 2))];
  const ui = render(<WorkflowRunPanel run={run}/>);
  const rows = () => [...ui.container.querySelectorAll('[data-workflow-node]')];
  const initial = rows();
  expect(initial.map(row => row.textContent)).toEqual([
    expect.stringContaining('fork'), expect.stringContaining('fork / zeta / repeat / body · 0.1'),
    expect.stringContaining('fork / alpha / repeat / body · 0.1'), expect.stringContaining('fork / zeta / repeat / body · 0.2'),
  ]);
  run.instances[2].state = { type: 'settled', outcome: 'failed' };
  run.instances.push(node('done'));
  ui.rerender(<WorkflowRunPanel run={run}/>);
  expect(rows().slice(0, 4)).toEqual(initial);
  expect(rows().map(row => row.getAttribute('data-workflow-node'))).toEqual(['fork', 'read', 'read', 'read', 'done']);
});
it('places the run next to its exact native call, without inheriting an adjacent Attempt or another conversation', () => {
  const run = workflowRun(), s = snapshot();
  if(run.id.origin.type!=="tool" || !run.tool_call_id)throw new Error("Tool-origin test fixture");
  const tool = nativeTool('workflow-native', 'file_checkup', {}, []); tool.call_id = run.tool_call_id;
  s.transcript.entries = [{ cursor: '1', turn_process: { conversation_id: s.conversation_id, attempt_id: run.id.origin.attempt_id, attempt_id_source: null, outcome: 'running', final_message_id_deferred: false, control_cursor: '1', message_count: 1, tool_call_count: 1 }, tool_calls: [tool], item: { type: 'message', message: { role: 'assistant', id: tool.message_id!, content: [{ type: 'tool_call', id: run.tool_call_id, name: 'file_checkup', tool_id: tool.tool_id!, arguments: {} }] } } }];
  const other = { ...run, id: { ...run.id, conversation_id: 'other' } };
  const ui = render(<AgentTranscript snapshot={s} workflows={{ revision: '1', runs: [run, other], omitted_runs: 0 }}/>);
  const panel = ui.container.querySelector('[data-workflow-run-id]')!;
  expect(ui.container.querySelectorAll('[data-workflow-run-id]')).toHaveLength(1);
  expect(panel.parentElement?.querySelector('[data-chat-anchor-key="message:' + tool.message_id + '"]')).toBeTruthy();
});
it('uses the DSH review card with readable content and explicit submit, requiring feedback for rejection', () => {
  const accepted: boolean[] = [], rejected: string[] = [], ui = render(<Review review={workflowReview()} title="Waiting for your response" disabled={false} submitting={false} accept={() => accepted.push(true)} reject={text => rejected.push(text)} cancel={() => {}}/>);
  expect(ui.getByRole('heading', { name: 'Review report' })).toBeTruthy();
  expect(ui.container.querySelector('pre')).toBeNull();
  expect(accepted).toEqual([]);
  fireEvent.click(ui.getByRole('radio', { name: 'Reject with feedback' }));
  const submit = ui.getByRole('button', { name: 'Submit' });
  expect((submit as HTMLButtonElement).disabled).toBe(true);
  fireEvent.change(ui.getByLabelText('Review feedback'), { target: { value: 'Fix the release version.' } });
  fireEvent.click(submit); expect(rejected).toEqual(['Fix the release version.']);
  fireEvent.click(ui.getByRole('radio', { name: 'Accept review' }));
  fireEvent.click(submit); expect(accepted).toEqual([true]);
  fireEvent.click(ui.getByRole('button', { name: 'Collapse the question card' }));
  expect(ui.queryByRole('heading', { name: 'Review report' })).toBeNull();
});
it('renders historical review and its exact settlement once, without raw interaction envelopes', () => {
  const s = snapshot(), review = workflowReview();
  s.transcript.entries = [{ cursor: '1', item: { type: 'interaction_requested', event_id: 'event-1', timestamp: '2026-10-20T00:00:00Z', attempt_id: 'a', turn_id: 't', interaction_id: 'review-1', subject: { type: 'review', review } } },
    { cursor: '2', item: { type: 'interaction_settled', event_id: 'event-2', timestamp: '2026-10-20T00:01:00Z', attempt_id: 'a', turn_id: 't', interaction_id: 'review-1', settlement: { type: 'reviewed', response: { instance: review.instance, subject_digest: 'digest', decision: { type: 'rejected', feedback: 'Please revise.' } } } } }];
  const ui = render(<AgentTranscript snapshot={s}/>);
  expect(ui.container.querySelectorAll('[data-interaction-audit]')).toHaveLength(1);
  fireEvent.click(ui.getByRole('button', { name: /Review · gate/ }));
  expect(ui.getByText('Please revise.')).toBeTruthy();
  expect(ui.container.textContent).not.toMatch(/interaction_requested|subject_digest|Historical interaction/);
});

it('annotates a running node only from the matching native pending review, without rewriting its execution state', () => {
  const run = workflowRun(); run.state = { type: 'running' }; run.instances[0].state = run.state; run.instances[4].state = run.state;
  const item = interaction('approval'); item.kind = 'review'; item.body!.kind = { type: 'review', subject_digest: 'native-digest', review: workflowReview() };
  const ui = render(<WorkflowRunPanel run={run} pending={[item]}/>);
  const gate = ui.container.querySelector('[data-workflow-node="gate"]')!;
  expect(gate.textContent).toContain('Waiting for review');
  expect(gate.getAttribute('data-node-state')).toBe('running');
  item.body!.kind.review.instance.visit = 2;
  ui.rerender(<WorkflowRunPanel run={run} pending={[item]}/>);
  expect(gate.textContent).not.toContain('Waiting for review');
  expect(run.instances[4].state).toEqual({ type: 'running' });
});
it('keeps exact Workflow inspection available after settlement and preserves keyboard focus', async () => {
  const run = workflowRun();
  const node = run.instances[1]; node.kind = 'agent'; node.child = 'finite-child'; node.child_agent_id = 'finite-agent'; node.state = { type: 'running' };
  const openWorkflow = vi.fn().mockResolvedValue(undefined);
  const context = { client: {} as AppServerClient, agents: [], metrics: {}, metricErrors: {}, opened: [], open: vi.fn(), close: vi.fn(), openWorkflow };
  const ui = render(<SubagentContext value={context}><WorkflowRunPanel run={run}/></SubagentContext>);
  const button = ui.getByRole('button', { name: 'View read' });
  act(() => button.focus()); fireEvent.click(button);
  expect(openWorkflow).toHaveBeenCalledWith('finite-agent', 'finite-child', 'read');
  await act(async () => {});
  node.state = { type: 'settled', outcome: 'completed' };
  run.state = node.state;
  ui.rerender(<SubagentContext value={context}><WorkflowRunPanel run={run}/></SubagentContext>);
  expect(ui.getByRole('button', { name: 'View read' }).getAttribute('aria-disabled')).toBeNull();
  expect(ui.getByRole('button', { name: /file_checkup/ }).getAttribute('aria-expanded')).toBe('true');
  fireEvent.click(button); expect(openWorkflow).toHaveBeenCalledTimes(2);
  await act(async () => {});
  act(() => button.blur());
  expect(ui.queryByRole('button', { name: 'View read' })).toBeNull();
  fireEvent.click(ui.getByRole('button', { name: /file_checkup/ }));
  expect(ui.getByRole('button', { name: 'View read' })).toBeTruthy();
  expect(ui.getByRole('button', { name: /file_checkup/ }).getAttribute('aria-expanded')).toBe('true');
});

it.each(['completed', 'failed', 'cancelled'] as const)('opens an already %s Workflow child on first visit and after remount', async outcome => {
  const run = workflowRun(); run.state = { type: 'settled', outcome };
  Object.assign(run.instances[1], { kind: 'agent', child: 'finite-child', child_agent_id: 'finite-agent', state: run.state });
  const openWorkflow = vi.fn().mockResolvedValue(undefined);
  const context = { client: {} as AppServerClient, agents: [], metrics: {}, metricErrors: {}, opened: [], open: vi.fn(), close: vi.fn(), openWorkflow };
  for (let visit = 0; visit < 2; visit++) {
    const ui = render(<SubagentContext value={context}><WorkflowRunPanel run={run}/></SubagentContext>);
    if (outcome === 'completed') fireEvent.click(ui.getByRole('button', { name: /file_checkup/ }));
    fireEvent.click(ui.getByRole('button', { name: 'View read' }));
    await act(async () => {}); ui.unmount();
  }
  expect(openWorkflow).toHaveBeenCalledTimes(2);
  expect(openWorkflow).toHaveBeenLastCalledWith('finite-agent', 'finite-child', 'read');
});

it('presents a Model call and its budget without creating a child Agent control', () => {
  const run = workflowRun();
  run.model_calls_consumed = 1;
  const instance = run.instances[1];
  instance.kind = 'model'; instance.state = { type: 'waiting', reason: 'model' };
  instance.auxiliary_call_id = 'bounded-model-call';
  const ui = render(<WorkflowRunPanel run={run}/>);
  const row = ui.container.querySelector('[data-workflow-node="read"]')!;
  expect(row.textContent).toContain('Model');
  expect(row.textContent).toContain('Waiting for model');
  expect(row.querySelector('button')).toBeNull();
  expect(ui.container.textContent).toContain('Agent calls 2 · Model calls 1');
});
