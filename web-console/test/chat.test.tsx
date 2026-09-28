import { cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { RuntimeFacts } from '../src/app/agent/Activity';
import { snapshot } from './fixture';
afterEach(cleanup);
const rich = '## Answer\n\n| Key | Value |\n| --- | --- |\n| one | two |\n\n- **Strong**\n\n```rust\nfn main() {}\n```\n\n$x^2$';
it('Chat uses the same rich Markdown through streaming and canonical settlement exactly once', () => {
  const live = { ...snapshot(), attempt: { attempt_id: 'a', phase: { type: 'running' as const }, turn: 1, in_flight: { message_id: 'answer', blocks: [{ type: 'text' as const, block_index: 0, text: rich }] } } };
  const ui = render(<AgentTranscript snapshot={live} />);
  expect(ui.getByRole('heading', { name: 'Answer' })).toBeTruthy();
  expect(ui.getByRole('table')).toBeTruthy();
  const committed = { ...live, messages: [{ role: 'assistant' as const, id: 'answer', content: [{ type: 'text' as const, text: rich }] }] };
  committed.transcript = { entries: committed.messages.map(message => ({ cursor: '1', item: { type: 'message', message } })) };
  ui.rerender(<AgentTranscript snapshot={committed} />);
  expect(ui.getAllByRole('heading', { name: 'Answer' })).toHaveLength(1);
  expect(ui.queryByLabelText('Streaming response')).toBeNull();
  expect(ui.getByRole('table')).toBeTruthy();
  expect(ui.container.querySelector('code')?.textContent).toContain('fn main()');
});
it('replacement drops stale partial content; incomplete Markdown stays visible', () => {
  const live = { ...snapshot(), attempt: { attempt_id: 'a', phase: { type: 'running' as const }, turn: 1, in_flight: { message_id: 'answer', blocks: [{ type: 'text' as const, block_index: 0, text: '```rust\nunfinished <tag>' }] } } };
  const ui = render(<AgentTranscript snapshot={live} />);
  expect(ui.container.textContent).toContain('unfinished <tag>');
  ui.rerender(<AgentTranscript snapshot={snapshot()} />);
  expect(ui.container.textContent).not.toContain('unfinished');
});
it('reasoning, refusal and native Tool identities are readable without fabricated runtime rows', () => {
  const s = snapshot(); s.messages = [{ role: 'assistant', id: 'm', content: [
    { type: 'reasoning', text: 'Reasoning fact' }, { type: 'refusal', text: 'Refusal fact' },
    { type: 'tool_call', id: 'call-1', tool_id: 'tool', name: 'Same title', arguments: {} },
    { type: 'tool_call', id: 'call-2', tool_id: 'tool', name: 'Same title', arguments: {} },
  ] }];
  s.transcript = { entries: s.messages.map(message => ({ cursor: '1', item: { type: 'message', message } })) };
  const ui = render(<><AgentTranscript snapshot={s} /><RuntimeFacts snapshot={s} /></>);
  expect(ui.getByText('Reasoning')).toBeTruthy(); expect(ui.getByText('Refusal fact')).toBeTruthy();
  expect(ui.getAllByText('Assembling Same title…')).toHaveLength(2);
  expect(ui.queryByText('Subagents')).toBeNull(); expect(ui.queryByText('Workflows')).toBeNull(); expect(ui.queryByText('Todo')).toBeNull();
});
it('durable user/assistant order and tool artifact galleries follow only transcript positions', () => {
  const s = snapshot();
  s.transcript = { entries: [
    { cursor: '1', item: { type: 'message', message: { role: 'user', id: 'u', source: 'human', content: [{ type: 'text', text: 'First user' }] } } },
    { cursor: '2', tool_calls: [{ message_id: 'a', block_index: 1, call_id: 'native-call', tool_id: 'image-tool', name: 'image-tool', state: { type: 'settled', arguments: '{}', result: { status: { type: 'success' }, duration_ms: 1, artifacts: [{ artifact_id: 'artifact_1', name: 'tool.png', mime_type: 'image/png' }] } } }], item: { type: 'message', message: { role: 'assistant', id: 'a', content: [{ type: 'text', text: 'Second assistant' }, { type: 'tool_call', id: 'native-call', tool_id: 'image-tool', name: 'image-tool', arguments: {} }] } } },
    { cursor: '3', item: { type: 'message', message: { role: 'tool', occurrence: { assistant_message_id: 'a', block_index: 1 }, id: 't', tool_call_id: 'native-call', tool_id: 'image-tool', result: { status: { type: 'success' }, duration_ms: 1, artifacts: [{ artifact_id: 'artifact_1', name: 'tool.png', mime_type: 'image/png' }] } } } },
  ] };
  const ui = render(<AgentTranscript snapshot={s} />);
  expect([...ui.container.querySelectorAll('[data-chat-anchor-key]')].map(node => node.getAttribute('data-chat-anchor-key'))).toEqual(['message:u', 'message:a']);
  fireEvent.click(ui.getByRole('button', { name: /image-tool/ }));
  expect(ui.getByText('tool.png')).toBeTruthy();
  expect(ui.container.querySelector('[data-tool-call-id="native-call"]')).toBeTruthy();
});
it('Job result galleries retain finite identities, including duplicate tool names', () => {
  const s = snapshot();
  s.jobs = ['exec-1', 'exec-2'].map(job_id => ({ job_id, tool_id: 'native', tool_name: 'Same name', state: 'succeeded', result: { status: { type: 'success' }, duration_ms: 1, artifacts: [{ artifact_id: `artifact-${job_id}`, name: `${job_id}.png`, mime_type: 'image/png' }, { artifact_id: `file-${job_id}`, name: `${job_id}.txt`, mime_type: 'text/plain' }] } }));
  const ui = render(<RuntimeFacts snapshot={s} />);
  for (const button of ui.getAllByRole('button', { name: /Same name/ })) fireEvent.click(button);
  for (const id of ['exec-1', 'exec-2']) {
    expect(ui.container.querySelector(`[data-job-id="${id}"]`)?.textContent).toContain(`${id}.png`);
    expect(ui.container.querySelector(`[data-job-id="${id}"]`)?.textContent).toContain(`${id}.txt`);
  }
});
it('Subagent and Workflow cards bind native identities and lifecycle facts without debug dumps', () => {
  const s = snapshot();
  s.agents = ['child-1', 'child-2'].map(activation_id => ({
    activation_id, agent_id: `agent-${activation_id}`, parent_agent_id: 'parent-agent', current_activation: activation_id, activation_state: 'running', child_conversation_id: `conv-${activation_id}`, agent: 'Same agent',
    definition_digest: 'private-digest', profile_digest: 'private-profile', state: 'active', started_at: '2026-09-15T00:00:00Z',
    observation: { revision: '1', activity: { type: 'waiting', on: { type: 'approval', tool_id: 'bash' } }, counters: { model_requests: 1, model_retries: 0, tool_executions: 0 } },
    workspace: { logical_workspace: '/private/workspace', isolation: { type: 'shared' }, resource_state: 'none' },
  }));
  s.workflows.runs = ['1', '2'].map(invocation => ({
    id: { conversation_id: 'conv', attempt_id: 'attempt', invocation }, workflow_id: 'Same workflow', program_digest: 'private-program', resource_revision: '1', tool_call_id: `call-${invocation}`,
    state: { type: 'waiting', reason: 'review' }, instances: [], omitted_instances: 0, steps_consumed: 2, steps_max: 10, agents_consumed: 1, candidate_users: 0,
  }));
  const workflow = s.workflows.runs[0];
  workflow.instances = ['review', 'verify'].map(node => ({
    block: { run: workflow.id, definition: { workflow_id: workflow.workflow_id, blocks: [] }, invocations: [] },
    node, visit: 1, kind: 'agent', state: { type: 'running' }, child: `workflow-${node}`,
  }));
  const ui = render(<RuntimeFacts snapshot={s} />);
  expect(ui.container.querySelectorAll('[data-agent-id]')).toHaveLength(2);
  expect(ui.container.querySelectorAll('[data-workflow-run-id]')).toHaveLength(2);
  expect(ui.getAllByText('Waiting for approval')).toHaveLength(2);
  expect(ui.getAllByText('Waiting for review')).toHaveLength(2);
  expect(ui.container.querySelectorAll('[data-workflow-activation-id]')).toHaveLength(2);
  const workflowChild = ui.container.querySelector('[data-workflow-activation-id="workflow-review"]');
  const child = ui.container.querySelector('[data-agent-id="agent-child-1"]');
  s.agents[0].state = 'inactive'; s.agents[0].activation_state = 'failed'; s.agents[0].current_activation = null; s.agents[0].detail = 'Native failure';
  s.workflows.runs[0].state = { type: 'settled', outcome: 'completed' };
  workflow.instances[0].state = { type: 'settled', outcome: 'completed' };
  ui.rerender(<RuntimeFacts snapshot={s} />);
  expect(ui.container.querySelector('[data-agent-id="agent-child-1"]')).toBe(child);
  expect(child?.textContent).toContain('Native failure');
  expect(ui.container.querySelector('[data-workflow-activation-id="workflow-review"]')).toBe(workflowChild);
  expect(workflowChild?.textContent).toContain('completed');
  expect(ui.getByText('completed')).toBeTruthy();
  expect(ui.container.querySelector('pre')).toBeNull();
  expect(ui.container.textContent).not.toMatch(/private-|Todo|Goal|Trace/);
});

it('one Agent row survives inactive settlement and a later activation, including reconnect replacement', () => {
  const s = snapshot();
  const agent = {
    agent_id: 'durable-child', parent_agent_id: 'parent-agent', activation_id: 'activation-a', current_activation: 'activation-a',
    child_conversation_id: 'child-conversation', agent: 'Worker', state: 'active' as const, activation_state: 'running' as const,
    definition_digest: 'definition', profile_digest: 'profile', started_at: '2026-09-15T00:00:00Z',
    observation: { revision: '1', activity: { type: 'awaiting_activity' as const }, counters: { model_requests: 0, model_retries: 0, tool_executions: 0 } },
    workspace: { logical_workspace: '/workspace', isolation: { type: 'shared' as const }, resource_state: 'none' as const },
  };
  s.agents = [agent];
  const ui = render(<RuntimeFacts snapshot={s}/>);
  const row = ui.container.querySelector('[data-agent-id="durable-child"]');
  expect(row?.getAttribute('data-activation-id')).toBe('activation-a');
  ui.rerender(<RuntimeFacts snapshot={{ ...s, agents: [{ ...agent, state: 'inactive', activation_state: 'succeeded', current_activation: null, detail: 'Final report committed' }] }}/>);
  expect(ui.container.querySelector('[data-agent-id="durable-child"]')).toBe(row);
  expect(row?.textContent).toContain('Inactive');
  expect(row?.textContent).toContain('Final report committed');
  expect(row?.hasAttribute('data-activation-id')).toBe(false);
  const resumed = { ...s, agents: [{ ...agent, activation_id: 'activation-b', current_activation: 'activation-b' }] };
  ui.rerender(<RuntimeFacts snapshot={resumed}/>);
  expect(ui.container.querySelector('[data-agent-id="durable-child"]')).toBe(row);
  expect(row?.getAttribute('data-activation-id')).toBe('activation-b');
  expect(row?.textContent).toContain('child-conversation');
  ui.rerender(<RuntimeFacts snapshot={structuredClone(resumed)}/>);
  expect(ui.container.querySelectorAll('[data-agent-id="durable-child"]')).toHaveLength(1);
  expect(ui.container.querySelector('[data-agent-id="durable-child"]')).toBe(row);
});

it('a finite Job retains its expanded output across proactive terminal snapshot replacement', () => {
  const s = snapshot();
  const job = { job_id: 'finite-job', tool_id: 'native.bash', tool_name: 'bash', state: 'running' as const };
  s.jobs = [job];
  const ui = render(<RuntimeFacts snapshot={s}/>);
  fireEvent.click(ui.getByRole('button', { name: /bash/ }));
  const row = ui.container.querySelector('[data-job-id="finite-job"]');
  ui.rerender(<RuntimeFacts snapshot={{ ...s, jobs: [{ ...job, state: 'succeeded', result: { status: { type: 'success' }, duration_ms: 1, content: [{ type: 'text', text: 'Settled output' }] } }] }}/>);
  expect(ui.container.querySelector('[data-job-id="finite-job"]')).toBe(row);
  expect(row?.textContent).toContain('Settled output');
  expect(ui.getByLabelText('Tool status').textContent).toBe('success');
});
