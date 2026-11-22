import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { RuntimeClientJob } from '../../protocol/app-server/v81';
import { BackgroundJobsSection } from '../src/app/agent/BackgroundJobs';
import { SessionPanel } from '../src/app/agent/SessionPanel';
import { SessionPanelToggle } from '../src/app/agent/SessionPanelToggle';
import { SessionPanelProvider } from '../src/app/agent/session-panel-state';
import { RuntimeFacts } from './runtime-facts';
import { traceRecord } from './trace-fixture';
import { Server, snapshot, domainReceipt } from './fixture';
import { RpcFailure } from '../src/client/errors';

afterEach(() => { cleanup(); localStorage.clear(); vi.useRealTimers(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
const running = (job_id: string): RuntimeClientJob => ({ job_id, tool_id: 'tool-bash', tool_name: job_id, state: 'running', bash: { command: `echo ${job_id}`, description: `Run ${job_id}` } });
const ended = (job: RuntimeClientJob): RuntimeClientJob => ({ ...job, state: 'succeeded', result: { status: { type: 'success' }, duration_ms: '2500', content: [{ type: 'text', text: `Output ${job.job_id}` }] } });
const toggleButton = () => document.querySelector('[data-session-panel-toggle]') as HTMLButtonElement;

function renderPanel(server: Server, sessionId = 'A', showChatView = true) {
  return render(
    <SessionPanelProvider>
      <SessionPanelToggle client={server.client} sessionId={sessionId} />
      {showChatView && <SessionPanel client={server.client} sessionId={sessionId} />}
    </SessionPanelProvider>,
  );
}

it('keeps Session panel toggle present without jobs and shows blue dot only when running and closed', async () => {
  const server = new Server();
  try {
    server.snapshots.set('A', { ...snapshot(), jobs: [] });
    await server.attached('A');
    renderPanel(server, 'A', true);
    expect(toggleButton()).toBeTruthy();
    expect(toggleButton().getAttribute('aria-disabled')).toBeNull();
    expect(toggleButton().getAttribute('aria-expanded')).toBe('false');
    expect(document.querySelector('[data-jobs-running]')).toBeNull();
    fireEvent.click(toggleButton());
    expect(toggleButton().getAttribute('aria-expanded')).toBe('true');
    const section = document.querySelector('[data-session-panel-section="background-jobs"]');
    expect(section).toBeTruthy();
    expect(within(section as HTMLElement).queryByRole('list')).toBeNull();
    fireEvent.click(toggleButton());
    expect(toggleButton().getAttribute('aria-expanded')).toBe('false');

    await act(async () => server.update('A', { ...snapshot(), jobs: [running('one')] }));
    expect(document.querySelector('[data-jobs-running]')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Session panel Background jobs running: 1' })).toBeTruthy();
    fireEvent.click(toggleButton());
    expect(document.querySelector('[data-jobs-running]')).toBeNull();
    expect(screen.getByRole('button', { name: 'Session panel' })).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Background jobs', level: 2 })).toBeTruthy();
  } finally { server.client.disconnect(); }
});

it('docks automatically when probe fits, flips pinned storage, and marks toggle unavailable outside Chat view', async () => {
  let probeCallback: ResizeObserverCallback | undefined;
  let observedElement: Element | undefined;
  vi.stubGlobal('ResizeObserver', class {
    constructor(callback: ResizeObserverCallback) { probeCallback = callback; }
    observe(element: Element) { observedElement = element; }
    unobserve() {}
    disconnect() {}
  });
  const server = new Server();
  try {
    server.snapshots.set('A', { ...snapshot(), jobs: [running('one')] });
    await server.attached('A');
    const ui = renderPanel(server, 'A', true);
    expect(document.querySelector('[data-session-panel]')).toBeNull();
    act(() => {
      probeCallback?.([{ target: observedElement!, contentRect: { width: 1 } as DOMRectReadOnly } as ResizeObserverEntry], {} as ResizeObserver);
    });
    expect(document.querySelector('[data-session-panel]')?.getAttribute('data-session-panel')).toBe('docked');
    expect(toggleButton().getAttribute('aria-expanded')).toBe('true');
    expect(document.querySelector('[data-jobs-running]')).toBeNull();

    fireEvent.click(toggleButton());
    expect(document.querySelector('[data-session-panel]')).toBeNull();
    expect(localStorage.getItem('dsh.sessionPanel.pinned')).toBe('false');
    expect(document.querySelector('[data-jobs-running]')).toBeTruthy();

    ui.rerender(
      <SessionPanelProvider>
        <SessionPanelToggle client={server.client} sessionId="A" />
      </SessionPanelProvider>,
    );
    expect(toggleButton().getAttribute('aria-disabled')).toBe('true');
    expect(toggleButton().getAttribute('aria-expanded')).toBe('false');
    fireEvent.keyDown(document.body, { key: 'Tab' });
    fireEvent.focus(toggleButton());
    expect(screen.getByRole('tooltip', { hidden: true }).textContent).toBe('Switch to the Chat view to show the session panel');
  } finally { server.client.disconnect(); }
});

it('moves settled tasks out of activity, folds their output, and clears only ended identities', async () => {
  const jobs = ['one', 'two', 'three'].map(running);
  const source = { ...snapshot(), jobs };
  const ui = render(<><BackgroundJobsSection jobs={jobs}/><RuntimeFacts snapshot={source}/></>);
  expect(screen.queryByRole('region', { name: 'Current activity' })).toBeNull();
  expect(screen.getAllByRole('region', { name: /^Job / })).toHaveLength(3);
  ui.rerender(<BackgroundJobsSection jobs={[ended(jobs[0]), ...jobs.slice(1)]}/>);
  expect(screen.queryByRole('region', { name: 'Job one' })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Finished 1' }));
  fireEvent.click(within(screen.getByRole('region', { name: 'Job one' })).getByRole('button', { expanded: false }));
  expect(screen.getByText('Output one')).toBeTruthy();
  expect(within(screen.getByRole('region', { name: 'Job one' })).getByText('2s')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Clear finished jobs' }));
  expect(screen.queryByRole('button', { name: 'Finished 1' })).toBeNull();
  expect(screen.getAllByRole('region', { name: /^Job / })).toHaveLength(2);
  ui.rerender(<BackgroundJobsSection jobs={jobs.map(ended)}/>);
  expect(screen.getByRole('button', { name: 'Finished 2' }).getAttribute('aria-expanded')).toBe('true');
  expect(screen.getAllByRole('region', { name: /^Job / })).toHaveLength(2);
  fireEvent.click(screen.getByRole('button', { name: 'Clear finished jobs' }));
  expect(screen.queryByRole('list')).toBeNull();
  ui.unmount();
  // A fresh observation reconstructs the native history with the same grouping.
  render(<BackgroundJobsSection jobs={jobs.map(ended)}/>);
  expect(screen.getByRole('button', { name: 'Finished 3' }).getAttribute('aria-expanded')).toBe('true');
});

it('keeps cancellation and terminal publication active, and treats every absorbing outcome as ended', async () => {
  const states: RuntimeClientJob['state'][] = ['starting', 'running', 'cancelling', 'publishing_terminal', 'succeeded', 'failed', 'denied', 'cancelled', 'timed_out', 'outcome_unknown'];
  render(<BackgroundJobsSection jobs={states.map(state => ({ ...running(state), state }))}/>);
  expect(screen.getAllByRole('region', { name: /^Job / })).toHaveLength(4);
  fireEvent.click(screen.getByRole('button', { name: 'Finished 6' }));
  expect(screen.getAllByRole('region', { name: /^Job / })).toHaveLength(10);
});

it('supports overlay Escape focus restoration and outside dismissal', async () => {
  const server = new Server();
  try {
    server.snapshots.set('A', { ...snapshot(), jobs: [running('one')] });
    await server.attached('A');
    renderPanel(server, 'A', true);
    fireEvent.click(toggleButton());
    const aside = document.querySelector('[data-session-panel="overlay"]') as HTMLElement;
    expect(aside).toBeTruthy();
    const rowButton = within(aside).getByRole('button', { name: /Show live output/ });
    rowButton.focus();
    fireEvent.keyDown(aside, { key: 'Escape' });
    expect(document.querySelector('[data-session-panel]')).toBeNull();
    expect(document.activeElement).toBe(toggleButton());
    fireEvent.click(toggleButton());
    expect(document.querySelector('[data-session-panel="overlay"]')).toBeTruthy();
    fireEvent.pointerDown(document.body);
    expect(document.querySelector('[data-session-panel]')).toBeNull();
  } finally { server.client.disconnect(); }
});

it('subscribes to native job updates and scopes cleared identities to their conversation', async () => {
  const server = new Server();
  try {
    const jobs = ['one', 'two'].map(running);
    server.snapshots.set('A', { ...snapshot(), jobs });
    await server.attached('A');
    const ui = renderPanel(server, 'A', true);
    fireEvent.click(toggleButton());
    await act(async () => server.update('A', { ...snapshot(), jobs: [ended(jobs[0]), jobs[1]] }));
    fireEvent.click(screen.getByRole('button', { name: 'Clear finished jobs' }));
    await act(async () => server.update('A', { ...snapshot(), jobs: jobs.map(ended) }));
    expect(screen.getByRole('button', { name: 'Finished 1' })).toBeTruthy();
    await server.attached('B');
    server.snapshots.set('B', { ...snapshot('B'), jobs: jobs.map(ended) });
    await act(async () => server.client.refresh('B'));
    ui.rerender(
      <SessionPanelProvider>
        <SessionPanelToggle client={server.client} sessionId="B" />
        <SessionPanel client={server.client} sessionId="B" />
      </SessionPanelProvider>,
    );
    fireEvent.click(toggleButton());
    expect(screen.getByRole('button', { name: 'Finished 2' })).toBeTruthy();
  } finally { server.client.disconnect(); }
});

it('keeps deferred task output during refresh and rejects a late read after source replacement', async () => {
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
    const ui = renderPanel(server, 'A', true);
    fireEvent.click(toggleButton());
    expect(server.requests.filter(value => value.request.method === 'job/status')).toHaveLength(0);
    await act(async () => fireEvent.click(within(screen.getByRole('region', { name: 'Job large' })).getByRole('button', { expanded: false })));
    expect(screen.getByText(/Retained output Retained output/)).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Load task details' })).toBeNull();
    server.held.add('conversation/checkpointValueRead');
    await act(async () => server.update('A', { ...snapshot(), jobs: [structuredClone(header)] }));
    expect(screen.getByText(/Retained output Retained output/)).toBeTruthy();
    expect(document.querySelector('[data-session-panel]')!.textContent).not.toContain('Loading');
    const pending = await server.waitFor('conversation/checkpointValueRead', 2);
    await act(async () => server.update('A', { ...snapshot(), jobs: [] }));
    await act(async () => server.reply(pending));
    expect(within(document.querySelector('[data-session-panel-section="background-jobs"]') as HTMLElement).queryByRole('list')).toBeNull();
    ui.unmount();
  } finally { server.client.disconnect(); }
});

it('offers one command panel at a time, and keeps a pending stop across hiding the panel', async () => {
  const server = new Server();
  try {
    const jobs = ['one', 'two'].map(running);
    server.snapshots.set('A', { ...snapshot(), jobs });
    await server.attached('A');
    server.held.add('job/cancel');
    server.handlers.set('job/output', () => ({ type: 'job_output', text: '', truncated: false }));
    server.handlers.set('job/cancel', () => ({ type: 'job_cancellation_accepted', job_id: 'one' }));
    renderPanel(server, 'A', true);
    fireEvent.click(toggleButton());
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Show live output of echo one' })));
    expect(screen.getByRole('button', { name: 'Copy' })).toBeTruthy();
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Show live output of echo two' })));
    expect(screen.getByRole('button', { name: 'Show live output of echo one' })).toBeTruthy();
    expect(document.querySelectorAll('[data-terminal]')).toHaveLength(1);
    fireEvent.click(screen.getByRole('button', { name: 'Stop task echo one' }));
    expect(server.requests.filter(row => row.request.method === 'job/cancel')).toHaveLength(0);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Click again to confirm' })); await server.waitFor('job/cancel', 1); });
    fireEvent.click(toggleButton());
    fireEvent.click(toggleButton());
    expect((screen.getByRole('button', { name: 'Stop task echo one' }) as HTMLButtonElement).disabled).toBe(true);
    await act(async () => server.reply(await server.waitFor('job/cancel', 1)));
    expect((screen.getByRole('button', { name: 'Stop task echo one' }) as HTMLButtonElement).disabled).toBe(true);
    expect(document.querySelector('[data-job-id="one"]')?.getAttribute('data-job-state')).toBe('running');
  } finally { server.client.disconnect(); }
});

it('orders live newest-started first and settled newest-finished first with start-order tie-break', async () => {
  const mk = (job_id: string, state: RuntimeClientJob['state']): RuntimeClientJob => ({
    job_id, tool_id: 'tool-bash', tool_name: 'bash', state, bash: { command: job_id, description: job_id },
    ...(state === 'succeeded' ? { result: { status: { type: 'success' }, duration_ms: '1000', content: [{ type: 'text', text: 'ok' }] } } : {}),
  });
  const t = (id: string, started_at: string, ended_at?: string) => traceRecord(1, { kind: 'background', native_id: id, timing: { started_at, ...(ended_at ? { ended_at } : {}) } });
  const jobs = [mk('live-old', 'running'), mk('live-new', 'running'), mk('done-b', 'succeeded'), mk('done-a', 'succeeded'), mk('done-newest', 'succeeded')];
  const records = [
    t('live-old', '2026-10-19T12:00:01Z'),
    t('live-new', '2026-10-19T12:00:05Z'),
    t('done-b', '2026-10-19T12:00:02Z', '2026-10-19T12:00:10Z'),
    t('done-a', '2026-10-19T12:00:01Z', '2026-10-19T12:00:10Z'),
    t('done-newest', '2026-10-19T12:00:03Z', '2026-10-19T12:00:20Z'),
  ];
  render(<BackgroundJobsSection jobs={jobs} records={records}/>);
  fireEvent.click(screen.getByRole('button', { name: 'Finished 3' }));
  const labels = [...document.querySelectorAll('[data-job-label]')].map(el => el.textContent);
  expect(labels).toEqual(['live-new', 'live-old', 'done-newest', 'done-a', 'done-b']);
});

it('folds groups past five rows and persists fold and clear choices per session across remount', async () => {
  const server = new Server();
  try {
    const live = Array.from({ length: 7 }, (_, i) => running(`live-${i}`));
    const settled = Array.from({ length: 7 }, (_, i) => ended(running(`done-${i}`)));
    server.snapshots.set('A', { ...snapshot(), jobs: [...live, ...settled] });
    await server.attached('A');
    const ui = renderPanel(server, 'A', true);
    expect(document.querySelector('[data-jobs-running]')).toBeTruthy();
    fireEvent.click(toggleButton());
    expect(document.querySelector('[data-jobs-running]')).toBeNull();
    expect(screen.getAllByRole('region', { name: /^Job / })).toHaveLength(5);
    const liveMore = screen.getByRole('button', { name: 'Show 2 more running (running background jobs)' });
    expect(liveMore.getAttribute('aria-expanded')).toBe('false');
    fireEvent.click(liveMore);
    expect(screen.getAllByRole('region', { name: /^Job / })).toHaveLength(7);
    expect(screen.getByRole('button', { name: 'Show less (running background jobs)' }).getAttribute('aria-expanded')).toBe('true');
    fireEvent.click(screen.getByRole('button', { name: 'Finished 7' }));
    expect(screen.getAllByRole('region', { name: /^Job / })).toHaveLength(12);
    fireEvent.click(screen.getByRole('button', { name: 'Show 2 more finished (finished background jobs)' }));
    expect(screen.getAllByRole('region', { name: /^Job / })).toHaveLength(14);
    ui.unmount();
    renderPanel(server, 'A', true);
    fireEvent.click(toggleButton());
    expect(screen.getAllByRole('region', { name: /^Job / })).toHaveLength(14);
    fireEvent.click(screen.getByRole('button', { name: 'Clear finished jobs' }));
    expect(screen.getAllByRole('region', { name: /^Job / })).toHaveLength(7);
  } finally { server.client.disconnect(); }
});

it('marks nonzero exit codes with warning dot and keeps outputless settled rows static', async () => {
  const exit3: RuntimeClientJob = { job_id: 'exit-3', tool_id: 'tool-bash', tool_name: 'bash', state: 'succeeded', bash: { command: 'exit 3', description: 'fail' }, result: { status: { type: 'success' }, duration_ms: '1000', exit_code: 3 } };
  const clean: RuntimeClientJob = { job_id: 'clean', tool_id: 'tool-bash', tool_name: 'bash', state: 'succeeded', bash: { command: 'true', description: 'ok' }, result: { status: { type: 'success' }, duration_ms: '1000', exit_code: 0 } };
  render(<BackgroundJobsSection jobs={[exit3, clean]}/>);
  expect(screen.getAllByRole('region', { name: 'Job bash' })).toHaveLength(2);
  const states = [...document.querySelectorAll('[data-job-id] [data-state]')].map(node => node.getAttribute('data-state'));
  expect(states).toEqual(['warning', 'done']);
  const cleanRegion = document.querySelector('[data-job-id="clean"]') as HTMLElement;
  expect(within(cleanRegion).queryByRole('button')).toBeNull();
});

it('streams live job output with gap/error notices and reveals hover card and marquee', async () => {
  vi.useFakeTimers({ now: 1_700_000_010_000 });
  vi.stubGlobal('matchMedia', vi.fn((query: string) => ({ matches: query === '(any-hover: hover)' })));
  const server = new Server();
  try {
    let call = 0;
    server.handlers.set('job/output', () => {
      call += 1;
      if (call === 1) return { type: 'job_output', text: 'streamed-line\n', truncated: true };
      throw new RpcFailure({ code: -32000, message: 'stream interrupted' });
    });
    const job = running('stream');
    server.snapshots.set('A', { ...snapshot(), jobs: [job] });
    await server.attached('A');
    const ui = renderPanel(server, 'A', true);
    fireEvent.click(toggleButton());
    const row = ui.baseElement.querySelector<HTMLElement>('[data-job-row]')!;
    const label = row.querySelector<HTMLElement>('[data-job-label]')!;
    Object.defineProperty(label, 'scrollWidth', { value: 300, configurable: true });
    Object.defineProperty(label, 'clientWidth', { value: 180, configurable: true });
    fireEvent.pointerEnter(row, { pointerType: 'mouse' });
    expect(label.hasAttribute('data-marquee')).toBe(true);
    expect(label.style.getPropertyValue('--dsh-marquee-range')).toBe('120px');
    act(() => { vi.advanceTimersByTime(500); });
    const card = document.querySelector('[data-session-panel-details]') as HTMLElement;
    expect(card).toBeTruthy();
    expect(card.textContent).toContain('stream');
    expect(card.textContent).toContain('running');
    const stop = screen.getByRole('button', { name: 'Stop task echo stream' });
    fireEvent.pointerEnter(stop, { pointerType: 'mouse' });
    expect(document.querySelector('[data-session-panel-details]')).toBeNull();
    fireEvent.pointerLeave(row, { pointerType: 'mouse' });
    expect(label.hasAttribute('data-marquee')).toBe(false);
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Show live output of echo stream' })));
    expect(screen.getByText('streamed-line')).toBeTruthy();
    expect(screen.getByText('… earlier output dropped …')).toBeTruthy();
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(screen.getByRole('alert').textContent).toContain('live output stream interrupted');
    expect(screen.getByText('streamed-line')).toBeTruthy();
  } finally { server.client.disconnect(); }
});
