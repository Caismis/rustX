import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { WorkflowRunPanel } from '../src/app/agent/WorkflowRunPanel';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { Review } from '../src/app/agent/Review';
import { workflowReview, workflowRun } from './workflow-data';
import { interaction, snapshot } from './fixture';
import { nativeTool } from './native-tool-cases';
afterEach(cleanup);

it('shows every native node while waiting, labels the budget correctly, and collapses only on native settlement', () => {
  const run = workflowRun(), ui = render(<WorkflowRunPanel run={run}/>);
  for (const name of ['read', 'triage', 'analyze', 'gate', 'loop', 'done']) expect(ui.getByText(name)).toBeTruthy();
  expect(ui.container.textContent).toContain('Steps used 9 · Limit 10 · Agent calls 2');
  expect(ui.container.textContent).not.toContain('9 / 10');
  run.steps_consumed = 10; ui.rerender(<WorkflowRunPanel run={run}/>);
  expect(ui.container.querySelector('[data-run-status]')?.getAttribute('data-run-status')).toBe('waiting');
  run.state = { type: 'settled', outcome: 'completed' }; run.instances[0].state = run.state;
  ui.rerender(<WorkflowRunPanel run={run}/>);
  expect(ui.getByRole('button').getAttribute('aria-expanded')).toBe('false');
  fireEvent.click(ui.getByRole('button')); fireEvent.click(ui.getByRole('button', { name: /Main flow/ }));
  expect(ui.getAllByText('Not run').length).toBeGreaterThan(0);
  expect(ui.container.querySelector('pre')).toBeNull();
});
it('keeps abnormal and loop outcomes visible and does not create Agent controls for finite activations', () => {
  const run = workflowRun(); run.state = { type: 'settled', outcome: 'failed' };
  run.instances[4].state = { type: 'settled', outcome: 'denied' };
  run.instances[5] = { ...run.instances[5], iteration: 3, iterations_max: 3, loop_exit: 'exhausted', checks_passed: false };
  run.instances[1].child = 'finite-activation';
  const ui = render(<WorkflowRunPanel run={run}/>);
  expect(ui.getByText(/Iteration limit reached/)).toBeTruthy();
  expect(ui.getByText(/Checks failed/)).toBeTruthy();
  expect(ui.container.querySelector('[data-workflow-activation-id="finite-activation"] button')).toBeNull();
});
it('places the run next to its exact native call, without inheriting an adjacent Attempt or another conversation', () => {
  const run = workflowRun(), s = snapshot();
  const tool = nativeTool('workflow-native', 'file_checkup', {}, []); tool.call_id = run.tool_call_id;
  s.transcript.entries = [{ cursor: '1', turn_process: { conversation_id: s.conversation_id, attempt_id: run.id.attempt_id, attempt_id_source: null, outcome: 'running', final_message_id_deferred: false, control_cursor: '1', message_count: 1, tool_call_count: 1 }, tool_calls: [tool], item: { type: 'message', message: { role: 'assistant', id: tool.message_id!, content: [{ type: 'tool_call', id: run.tool_call_id, name: 'file_checkup', tool_id: tool.tool_id!, arguments: {} }] } } }];
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
it('defers a clean collapse while a block header owns focus and lets the reader close it once', () => {
  const run = workflowRun(), ui = render(<WorkflowRunPanel run={run}/>);
  act(() => ui.getByRole('button', { name: /Main flow/ }).focus());
  run.state = { type: 'settled', outcome: 'completed' }; run.instances[0].state = run.state;
  ui.rerender(<WorkflowRunPanel run={run}/>);
  const root = ui.getByRole('button', { name: /file_checkup/ });
  expect(root.getAttribute('aria-expanded')).toBe('true');
  expect(fireEvent.mouseDown(root)).toBe(false);
  fireEvent.click(root);
  expect(root.getAttribute('aria-expanded')).toBe('false');
});
