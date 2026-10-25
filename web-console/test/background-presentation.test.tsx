import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { ForegroundToolExecution, MessageBlock } from '../../protocol/app-server/v57';
import { Tool } from '../src/app/agent/Tool';
import { InboundMessage } from '../src/app/agent/InboundMessage';
import { backgroundNotice } from '../src/bindings/jobs';
import { toolCard } from '../src/bindings/tools';
import { BackgroundJobMenu } from '../src/app/agent/BackgroundJobs';
import { traceRecord } from './trace-fixture';

afterEach(() => { cleanup(); vi.useRealTimers(); });
const receipt = { job_id: 'job-one', state: 'starting', tool: 'bash', output_path: '/workspace/output.txt', note: 'Native execution continuation instructions' };
function tool(value: unknown = receipt, name = 'bash', id = 'tool-bash'): ForegroundToolExecution {
  return { committed_result_deferred: false, occurrence: { locator: { type: 'by_message_id', message_id: 'assistant' }, block_index: 0 }, assembly_complete: true, message_id: 'assistant', block_index: 0, call_id: 'call-one', name, tool_id: id,
    state: { type: 'settled', arguments: '{"command":"sleep 5","execution_mode":"background"}', result: { status: { type: 'success' }, duration_ms: '0', content: [{ type: 'json', value: value as never }] } } };
}

it('renders a successful background launch as a static command receipt, without process-exit facts', () => {
  const launch = tool();
  expect(toolCard(launch)).toMatchObject({ background: true, jobId: 'job-one', state: 'success' });
  const ui = render(<Tool tool={launch}/>);
  expect(ui.getByLabelText('Tool status').textContent).toBe('Background task started');
  fireEvent.click(ui.getByRole('button', { expanded: false }));
  expect(ui.container.querySelector('[data-terminal][data-background]')).toBeTruthy();
  expect(ui.container.querySelector('[data-terminal] [data-state]')).toBeNull();
  expect(ui.queryByText('No output')).toBeNull();
  expect(ui.container.textContent).toContain('sleep 5');
  expect(ui.container.textContent).not.toContain(receipt.note);
  expect(ui.getByRole('button', { name: 'Copy' })).toBeTruthy();
});

it('keeps a refused or failed launch on the ordinary diagnostic path', () => {
  const failed = tool();
  failed.state = { type: 'settled', arguments: failed.state.arguments, result: { status: { type: 'denied', reason: 'Permission refused' }, duration_ms: '0' } };
  expect(toolCard(failed).background).toBe(false);
  const ui = render(<Tool tool={failed}/>);
  fireEvent.click(ui.getByRole('button', { expanded: false }));
  expect(ui.container.textContent).toContain('Permission refused');
  expect(ui.container.querySelector('[data-background]')).toBeNull();
});

it('shows recorded Job failure independently of a successful status/wait/cancel Tool receipt', () => {
  const ui = render(<Tool tool={tool({ job_id: 'job-one', tool: 'bash', state: 'failed', result: { content: [{ type: 'text', text: 'Command failed' }] } }, 'job_wait', 'tool-job_wait')}/>);
  expect(ui.container.textContent).toContain('failed');
  fireEvent.click(ui.getByRole('button', { expanded: false }));
  expect(ui.getByText('Command failed')).toBeTruthy();
  expect(ui.container.querySelectorAll('[data-tool-details]')).toHaveLength(1);
  expect(ui.queryByText('Full task record')).toBeNull();
  expect(ui.queryByText('Recorded task result')).toBeNull();
  expect(ui.getByRole('button', { name: 'Copy' })).toBeTruthy();
  expect(ui.queryByRole('button', { name: 'Cancel Job' })).toBeNull();
});

it('recognizes only exact runtime terminal notices and preserves terminal output', () => {
  const message: Extract<MessageBlock, { role: 'user' }> = { role: 'user', id: 'background-job-one-terminal', source: 'runtime', kind: 'message', content: [{ type: 'text', text: 'Background execution job-one (bash) settled: outcome_unknown\n\nResult:\nExternal outcome was not proven' }] };
  const ui = render(<InboundMessage message={message}/>);
  expect(ui.container.querySelector('[data-inbound-source="job"]')).toBeTruthy();
  fireEvent.click(ui.getByRole('button', { expanded: false }));
  expect(ui.getByText('External outcome was not proven')).toBeTruthy();
  expect(ui.container.textContent).not.toContain('Background execution job-one');
  expect(backgroundNotice({ ...message, source: 'human' })).toBeUndefined();
  expect(backgroundNotice({ ...message, id: 'ordinary-context' })).toBeUndefined();
  expect(backgroundNotice({ ...message, content: [{ type: 'text', text: 'Background execution job-one (bash) settled: publishing_terminal' }] })).toBeUndefined();
});

it('uses a matching native Trace start for the open-menu clock and freezes at terminal result duration', async () => {
  vi.useFakeTimers(); vi.setSystemTime(new Date('2026-10-19T12:00:05Z'));
  const job = { job_id: 'job-one', tool_name: 'bash', state: 'running' as const };
  const records = [traceRecord(1, { kind: 'background', native_id: job.job_id, timing: { started_at: '2026-10-19T12:00:00Z' } })];
  const ui = render(<BackgroundJobMenu jobs={[job]} records={records}/>);
  fireEvent.click(ui.getByRole('button', { name: 'Background tasks' }));
  expect(screen.getByText('5s')).toBeTruthy();
  await act(async () => vi.advanceTimersByTime(2000));
  expect(screen.getByText('7s')).toBeTruthy();
  ui.rerender(<BackgroundJobMenu jobs={[{ ...job, state: 'succeeded', result: { status: { type: 'success' }, duration_ms: '6000' } }]} records={records}/>);
  await act(async () => vi.advanceTimersByTime(3000));
  expect(screen.getByText('6s')).toBeTruthy();
});
