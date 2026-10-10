import { cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { ForegroundToolExecution } from '../../protocol/app-server/v41';
import { subagentToolDetails } from '../src/app/agent/subagent-tool-details';
import { DomainActivity } from '../src/app/agent/DomainActivity';
afterEach(cleanup);
const call = (name: string, input: unknown, result: unknown): ForegroundToolExecution => ({
  call_id: 'call', tool_id: `tool-${name}`, name, message_id: 'message', block_index: 0,
  state: { type: 'settled', arguments: JSON.stringify(input), result: { status: { type: 'success' }, duration_ms: 1, content: [{ type: 'json', value: result as never }] } },
});
it('creation is a start receipt, not the child terminal outcome', () => {
  const details = subagentToolDetails(call('subagent', { title: 'Inspect source tree', task: 'Inspect source' }, { agent_id: 'child', activation_id: 'first', state: 'active', agent: 'explore' }));
  expect(details.items).toEqual([{ agentId: 'child', name: 'Inspect source tree', text: 'Inspect source', status: 'common:subagents.started', tone: 'ongoing' }]);
});
it('list reads recorded owner state and retains the native bound', () => {
  const details = subagentToolDetails(call('list_agents', {}, { returned: 2, matched: 3, truncated: true, agents: [{ agent_id: 'a', agent: 'explore', title: 'First', state: 'inactive' }, { agent_id: 'b', agent: 'explore', title: 'Second', state: 'active' }] }));
  expect(details.count).toBe(2);
  expect(details.truncated).toEqual({ returned: 2, matched: 3 });
  expect(details.items.map(item => item.status)).toEqual(['common:subagents.inactive', 'common:subagents.active']);
});
it('wait distinguishes inactive from a successful or cancelled activation', () => {
  expect(subagentToolDetails(call('wait_agent', { agent_id: 'a' }, { agent_id: 'a', outcome: null })).items[0]?.status).toBe('common:subagents.inactive');
  expect(subagentToolDetails(call('wait_agent', { agent_id: 'a' }, { agent_id: 'a', outcome: 'succeeded' })).items[0]?.status).toBe('common:state.succeeded');
  expect(subagentToolDetails(call('wait_agent', { agent_id: 'a' }, { agent_id: 'a', outcome: 'interrupted' })).items[0]?.status).toBe('common:state.interrupted');
  expect(subagentToolDetails(call('interrupt_agent', { agent_id: 'a' }, { agent_id: 'a', outcome: 'cancelled' })).items[0]?.status).toBe('common:state.cancelled');
});
it('message acceptance is a delivery receipt and incomplete arguments invent no receipt', () => {
  expect(subagentToolDetails(call('send_message', { agent_id: 'a', message: 'Check the source' }, { agent_id: 'a', resumed: true })).items[0]?.status).toBe('common:subagents.accepted');
  const tool = call('subagent', {}, {}); tool.state = { type: 'assembled', arguments: '{' };
  expect(subagentToolDetails(tool)).toEqual({ target: undefined, task: undefined, items: [] });
});
it('expanded list has human details and exposes raw JSON only on explicit inspection', () => {
  const tool = call('list_agents', {}, { returned: 1, matched: 1, truncated: false, agents: [{ agent_id: 'a', agent: 'explore', title: 'Research', state: 'active' }] });
  const ui = render(<DomainActivity tool={tool}/>);
  fireEvent.click(ui.getByRole('button', { name: /Agents/ }));
  expect(ui.getByText('Research')).toBeTruthy();
  expect(ui.getByText('Running')).toBeTruthy();
  expect(ui.container.querySelector('pre')).toBeNull();
  fireEvent.click(ui.getByRole('button', { name: 'View' }));
  expect(ui.container.querySelector('pre')?.textContent).toContain('"agents"');
  fireEvent.click(ui.getByRole('button', { name: 'View' }));
  expect(ui.container.querySelector('pre')).toBeNull();
});
it('empty list is shown explicitly', () => {
  const ui = render(<DomainActivity tool={call('list_agents', {}, { returned: 0, matched: 0, truncated: false, agents: [] })}/>);
  fireEvent.click(ui.getByRole('button', { name: /Agents/ }));
  expect(ui.getByText('No subagents')).toBeTruthy();
});

it('child delivery metadata remains readable when no child preview authority exists', async () => {
  const { PresentedFileCard } = await import('../src/presentation/attachments/PresentedFileCard');
  const ui = render(<PresentedFileCard file={{ key: 'child-report', name: 'report.md', path: '/child/report.md' }} actions={null}/>);
  expect(ui.getByText('report.md')).toBeTruthy();
  expect((ui.getByRole('button', { name: /report.md/ }) as HTMLButtonElement).disabled).toBe(true);
});
