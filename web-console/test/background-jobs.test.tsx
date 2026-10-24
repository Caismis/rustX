import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { RuntimeClientJob } from '../../protocol/app-server/v57';
import { BackgroundJobMenu, BackgroundJobs } from '../src/app/agent/BackgroundJobs';
import { RuntimeFacts } from './runtime-facts';
import { Server, snapshot, domainReceipt } from './fixture';

afterEach(cleanup);
const running = (job_id: string): RuntimeClientJob => ({ job_id, tool_id: 'tool-bash', tool_name: job_id, state: 'running', bash: { command: `echo ${job_id}`, description: `Run ${job_id}` } });
const ended = (job: RuntimeClientJob): RuntimeClientJob => ({ ...job, state: 'succeeded', result: { status: { type: 'success' }, duration_ms: '2500', content: [{ type: 'text', text: `Output ${job.job_id}` }] } });
const trigger = () => screen.getByRole('button', { name: 'Background tasks' });

it('moves settled tasks out of activity, folds their output, and clears only ended identities', async () => {
  const jobs = ['one', 'two', 'three'].map(running);
  const source = { ...snapshot(), jobs };
  const ui = render(<><BackgroundJobMenu jobs={jobs}/><RuntimeFacts snapshot={source}/></>);
  expect(screen.queryByRole('region', { name: 'Current activity' })).toBeNull();
  expect(trigger().textContent).toContain('3 background tasks running');
  await act(async () => fireEvent.click(trigger()));
  expect(screen.getAllByRole('region', { name: /^Job / })).toHaveLength(3);
  ui.rerender(<BackgroundJobMenu jobs={[ended(jobs[0]), ...jobs.slice(1)]}/>);
  expect(trigger().textContent).toContain('2 background tasks running');
  expect(screen.queryByRole('region', { name: 'Job one' })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Ended 1' }));
  fireEvent.click(within(screen.getByRole('region', { name: 'Job one' })).getByRole('button', { expanded: false }));
  expect(screen.getByText('Output one')).toBeTruthy();
  expect(within(screen.getByRole('region', { name: 'Job one' })).getByText('2s')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Clear ended tasks' }));
  expect(screen.queryByRole('button', { name: 'Ended 1' })).toBeNull();
  expect(screen.getAllByRole('region', { name: /^Job / })).toHaveLength(2);
  ui.rerender(<BackgroundJobMenu jobs={jobs.map(ended)}/>);
  expect(trigger().textContent).toContain('2 background tasks');
  expect(screen.getByRole('button', { name: 'Ended 2' }).getAttribute('aria-expanded')).toBe('true');
  expect(screen.getAllByRole('region', { name: /^Job / })).toHaveLength(2);
  fireEvent.click(screen.getByRole('button', { name: 'Clear ended tasks' }));
  expect(screen.queryByRole('button', { name: 'Background tasks' })).toBeNull();
  expect(screen.queryByRole('dialog')).toBeNull();
  ui.unmount();
  // A fresh observation reconstructs the native history with the same grouping.
  render(<BackgroundJobMenu jobs={jobs.map(ended)}/>);
  expect(trigger().textContent).toContain('3 background tasks');
  await act(async () => fireEvent.click(trigger()));
  expect(screen.getByRole('button', { name: 'Ended 3' }).getAttribute('aria-expanded')).toBe('true');
});

it('keeps cancellation and terminal publication active, and treats every absorbing outcome as ended', async () => {
  const states: RuntimeClientJob['state'][] = ['starting', 'running', 'cancelling', 'publishing_terminal', 'succeeded', 'failed', 'denied', 'cancelled', 'timed_out', 'outcome_unknown'];
  render(<BackgroundJobMenu jobs={states.map(state => ({ ...running(state), state }))}/>);
  expect(trigger().textContent).toContain('4 background tasks running');
  await act(async () => fireEvent.click(trigger()));
  expect(screen.getAllByRole('region', { name: /^Job / })).toHaveLength(4);
  fireEvent.click(screen.getByRole('button', { name: 'Ended 6' }));
  expect(screen.getAllByRole('region', { name: /^Job / })).toHaveLength(10);
});

it('supports keyboard entry, Escape focus restoration and outside dismissal', async () => {
  render(<BackgroundJobMenu jobs={[running('one')]}/>);
  trigger().focus();
  await act(async () => fireEvent.keyDown(trigger(), { key: 'ArrowDown' }));
  expect(screen.getByRole('dialog')).toBeTruthy();
  fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(document.activeElement).toBe(trigger());
  await act(async () => fireEvent.click(trigger()));
  fireEvent.pointerDown(document.body);
  expect(screen.queryByRole('dialog')).toBeNull();
});

it('subscribes to native job updates and scopes cleared identities to their conversation', async () => {
  const server = new Server();
  try {
    const jobs = ['one', 'two'].map(running);
    server.snapshots.set('A', { ...snapshot(), jobs });
    await server.attached('A');
    const ui = render(<BackgroundJobs client={server.client} sessionId="A"/>);
    await act(async () => fireEvent.click(trigger()));
    await act(async () => server.update('A', { ...snapshot(), jobs: [ended(jobs[0]), jobs[1]] }));
    expect(trigger().textContent).toContain('1 background tasks running');
    fireEvent.click(screen.getByRole('button', { name: 'Clear ended tasks' }));
    await act(async () => server.update('A', { ...snapshot(), jobs: jobs.map(ended) }));
    expect(trigger().textContent).toContain('1 background tasks');
    await server.attached('B');
    server.snapshots.set('B', { ...snapshot('B'), jobs: jobs.map(ended) });
    await act(async () => server.client.refresh('B'));
    ui.rerender(<BackgroundJobs client={server.client} sessionId="B"/>);
    expect(trigger().textContent).toContain('2 background tasks');
  } finally { server.client.disconnect(); }
});

it('loads deferred historical output on demand and rejects a late read after source replacement', async () => {
  const server = new Server();
  try {
    const original = ended(running('large'));
    original.result!.content = [{ type: 'text', text: 'Retained output '.repeat(500) }];
    const header = { ...original, result: undefined, deferred_fields: [['result']] };
    server.snapshots.set('A', { ...snapshot(), jobs: [header] });
    await server.attached('A');
    server.handlers.set('job/status', () => domainReceipt(`jobs.${header.job_id}`, [header]));
    server.handlers.set('conversation/checkpointValueRead', request => {
      if (request.method !== 'conversation/checkpointValueRead') throw new Error('Wrong read');
      const text = JSON.stringify(original);
      return { type: 'checkpoint_value_chunk', chunk: { cut: request.params.cut, domain: request.params.domain, item_offset: request.params.item_offset, path: [], offset: 0, readable_end: text.length, next_offset: null, representation: 'canonical_json_v1', bytes_base64: btoa(text) } };
    });
    const ui = render(<BackgroundJobs client={server.client} sessionId="A"/>);
    await act(async () => fireEvent.click(trigger()));
    expect(server.requests.filter(value => value.request.method === 'job/status')).toHaveLength(0);
    await act(async () => fireEvent.click(within(screen.getByRole('region', { name: 'Job large' })).getByRole('button', { expanded: false })));
    expect(screen.getByText(/Retained output Retained output/)).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Load task details' })).toBeNull();
    server.held.add('conversation/checkpointValueRead');
    await act(async () => server.update('A', { ...snapshot(), jobs: [structuredClone(header)] }));
    expect(screen.queryByText(/Retained output Retained output/)).toBeNull();
    const pending = await server.waitFor('conversation/checkpointValueRead', 2);
    await act(async () => server.update('A', { ...snapshot(), jobs: [] }));
    await act(async () => server.reply(pending));
    expect(screen.queryByRole('button', { name: 'Background tasks' })).toBeNull();
    ui.unmount();
  } finally { server.client.disconnect(); }
});

it('offers one command panel at a time, and keeps a pending stop across closing the menu', async () => {
  const server = new Server();
  try {
    const jobs = ['one', 'two'].map(running);
    server.snapshots.set('A', { ...snapshot(), jobs });
    await server.attached('A');
    server.held.add('job/cancel');
    server.handlers.set('job/cancel', () => ({ type: 'job_cancellation_accepted', job_id: 'one' }));
    render(<BackgroundJobs client={server.client} sessionId="A"/>);
    await act(async () => fireEvent.click(trigger()));
    fireEvent.click(screen.getByRole('button', { name: 'Show output of echo one' }));
    expect(screen.getByRole('button', { name: 'Copy' })).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Show output of echo two' }));
    expect(screen.getByRole('button', { name: 'Show output of echo one' })).toBeTruthy();
    expect(document.querySelectorAll('[data-terminal]')).toHaveLength(1);
    fireEvent.click(screen.getByRole('button', { name: 'Stop task echo one' }));
    expect(server.requests.filter(row => row.request.method === 'job/cancel')).toHaveLength(0);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Click again to confirm' })); await server.waitFor('job/cancel', 1); });
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
    await act(async () => fireEvent.click(trigger()));
    expect((screen.getByRole('button', { name: 'Stop task echo one' }) as HTMLButtonElement).disabled).toBe(true);
    await act(async () => server.reply(await server.waitFor('job/cancel', 1)));
    expect((screen.getByRole('button', { name: 'Stop task echo one' }) as HTMLButtonElement).disabled).toBe(true);
    expect(document.querySelector('[data-job-id="one"]')?.getAttribute('data-job-state')).toBe('running');
  } finally { server.client.disconnect(); }
});
