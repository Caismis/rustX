import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { Message } from '../src/app/agent/Message';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { RuntimeFacts } from '../src/app/agent/Activity';
import { SubagentScope, SubagentHeader, SubagentSurface } from '../src/app/agent/Subagents';
import { cellKind } from '../src/app/trajectory/TrajectoryCell';
import { Server, snapshot } from './fixture';
import type { RuntimeClientAgent, MessageBlock } from '../../protocol/app-server/v37';
const servers: Server[] = [];
afterEach(() => { cleanup(); for (const server of servers.splice(0)) server.client.disconnect(); });
const agent: RuntimeClientAgent = { agent_id: 'child', agent: 'Research', parent_agent_id: 'root', child_conversation_id: 'child-conversation', activation_id: 'activation', state: 'inactive', activation_state: 'succeeded', definition_digest: 'd', profile_digest: 'p', started_at: '2026-10-08T00:00:00Z', observation: { revision: '1', activity: { type: 'awaiting_activity' }, counters: { model_requests: 1, model_retries: 0, tool_executions: 0 } }, workspace: { logical_workspace: '/workspace', isolation: { type: 'shared' }, resource_state: 'none' } };
const report: MessageBlock = { role: 'user', id: 'return', source: { agent: { agent_id: 'child' } }, content: [{ type: 'text', text: '**Research result**' }] };
it('agent return is a context disclosure, while a human message remains a user bubble', () => {
  const s = snapshot(); s.transcript = { entries: [{ cursor: '1', item: { type: 'message', message: report } }] };
  const ui = render(<AgentTranscript snapshot={s}/>);
  expect(ui.container.querySelector('[data-inbound-source="agent"]')).toBeTruthy();
  expect(ui.container.querySelector('[data-user-message]')).toBeNull();
  expect(ui.queryByLabelText('Message actions')).toBeNull();
  fireEvent.click(ui.getByRole('button', { name: /Message from/ }));
  expect(ui.container.querySelector('strong')?.textContent).toBe('Research result');
  ui.rerender(<Message message={{ ...report, source: 'human' }}/>);
  expect(ui.container.querySelector('[data-inbound-source]')).toBeNull();
});
it('agents move out of the message footer and open through the header without losing parent draft', async () => {
  const server = new Server(); servers.push(server); await server.attached('A');
  server.snapshots.get('A')!.agents = [agent]; await server.client.refresh('A');
  const ui = render(<SubagentScope client={server.client} sessionId="A"><SubagentHeader title="Parent"/><SubagentSurface><input aria-label="Root draft" defaultValue="keep me"/><RuntimeFacts snapshot={server.snapshots.get('A')!}/></SubagentSurface></SubagentScope>);
  expect(ui.queryByLabelText('Agent Research')).toBeNull();
  fireEvent.click(ui.getByRole('button', { name: 'Subagents' }));
  await act(async () => { fireEvent.click(ui.getByRole('menuitem', { name: /Research/ })); });
  expect(ui.getByLabelText('Agent Research')).toBeTruthy();
  expect(ui.queryByRole('textbox', { name: 'Root draft' })).toBeNull();
  fireEvent.click(ui.getByRole('button', { name: 'Parent' }));
  expect((ui.getByRole('textbox', { name: 'Root draft' }) as HTMLInputElement).value).toBe('keep me');
});
it('trajectory classifies native agent inbound records as context', () => {
  expect(cellKind({ type: 'RecordRow', record: { kind: 'user', agent_id: 'child' } } as Parameters<typeof cellKind>[0])).toBe('context');
});
