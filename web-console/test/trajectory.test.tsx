/* Copyright (c) 2026 DeepSeek. MIT. Adapted interaction contracts; see PROVENANCE.md. */
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Trajectory } from '../src/app/trajectory/Trajectory';
import { completeTraceDetail, replaceTrace, refreshTrace, selectTrace, type TraceCache } from '../src/client/trace';
import { requestDetail, toolDetail, traceRecord, traceTool } from './trace-fixture';
import { App } from '../src/app/App';
import { Server, snapshot } from './fixture';

let server: Server | undefined;

beforeEach(() => {
  localStorage.clear();
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(360);
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(800);
  vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(360);
  // jsdom performs no layout, so the ledger's scroll height is modelled from
  // what it actually renders: the virtualizer's explicit total when it owns
  // the window, otherwise one fixed row height per mounted row.
  vi.spyOn(HTMLElement.prototype, 'scrollHeight', 'get').mockImplementation(function (this: HTMLElement) {
    const virtual = this.querySelector<HTMLElement>(':scope > div[style*="height"]');
    const explicit = Number.parseFloat(virtual?.style.height ?? '');
    if (Number.isFinite(explicit) && explicit > 0) return explicit;
    const rows = this.querySelectorAll('[data-trace-id]').length;
    return rows > 0 ? rows * 30 : 360;
  });
  Object.defineProperty(HTMLElement.prototype, 'scrollTo', {
    configurable: true,
    value: function (this: HTMLElement, options: ScrollToOptions) {
      this.scrollTop = options.top ?? 0;
    },
  });
});

afterEach(() => {
  server?.client.disconnect();
  server = undefined;
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const noop = () => {};
function renderTrajectory(cache: TraceCache, onLoadDetail: (id: string) => void = noop) {
  return render(
    <Trajectory cache={cache} loadEarlier={noop} latest={noop} onSelect={noop} onLoadDetail={onLoadDetail} />,
  );
}
const cacheOf = (records = [traceRecord(0), traceRecord(1)], next_cursor: string | null = null) =>
  replaceTrace({ records, next_cursor });

it('groups rows by the Attempt and Step the server resolved, and previews their content', () => {
  renderTrajectory(
    cacheOf([
      traceRecord(0, { preview: { text: 'first request', truncated: false } }),
      traceRecord(1, { location: { attempt_id: 'attempt-a', step_id: '2' } }),
    ]),
  );
  const ledger = screen.getByRole('table', { name: 'Trace ledger' });
  // One section opens for the Attempt the server resolved; both Steps of that
  // Attempt sit inside it.
  expect(within(ledger).getByText('Attempt 1')).toBeDefined();
  expect(within(ledger).getByText('first request')).toBeDefined();
  expect(ledger.querySelectorAll('[data-trace-id]')).toHaveLength(2);
  expect(within(ledger).getByLabelText('Fold Step 1')).toBeDefined();
  expect(within(ledger).getByLabelText('Fold Step 2')).toBeDefined();
});

it('a proposed ToolCall never renders as an execution, and a started Tool says so', () => {
  const proposal = traceRecord(0, {
    kind: 'assistant',
    request: null,
    preview: { text: 'calling bash', truncated: false },
    calls: [{ call_id: 'call-1', tool_id: 'tool-bash', name: 'bash' }],
  });
  renderTrajectory(cacheOf([proposal, traceTool(1)]));
  fireEvent.click(screen.getByText('calling bash'));
  const inspector = screen.getByLabelText('Trace record inspector');
  expect(within(inspector).getByText(/proves assembly/)).toBeDefined();
  expect(within(inspector).queryByText(/durable start fact/)).toBeNull();

  fireEvent.click(screen.getByText('ls -la'));
  expect(
    within(screen.getByLabelText('Trace record inspector')).getByText(/durable start fact/),
  ).toBeDefined();
});

it('an unterminated record shows no duration and no generation metrics', () => {
  renderTrajectory(
    cacheOf([traceRecord(0, { state: 'running', timing: { started_at: '2026-09-15T00:00:00Z' } })]),
  );
  fireEvent.click(screen.getByText('historical-model-0'));
  const inspector = screen.getByLabelText('Trace record inspector');
  fireEvent.click(within(inspector).getByRole('tab', { name: 'Timing' }));
  expect(within(inspector).getAllByText('Unavailable').length).toBeGreaterThan(0);
  expect(within(inspector).getByText(/No settled generation evidence/)).toBeDefined();
});

it('settled generation evidence renders TTFT, decode duration and throughput', () => {
  renderTrajectory(
    cacheOf([
      traceRecord(0, {
        request: {
          ...traceRecord(0).request!,
          generation: {
            timeline: null,
            ttft_ms: '320',
            generation_ms: '1280',
            terminal_ms: '1600',
            output_tokens_per_second: 93.75,
          },
        },
      }),
    ]),
  );
  fireEvent.click(screen.getByText('historical-model-0'));
  const inspector = screen.getByLabelText('Trace record inspector');
  fireEvent.click(within(inspector).getByRole('tab', { name: 'Timing' }));
  expect(within(inspector).getByText('320 ms')).toBeDefined();
  expect(within(inspector).getByText('1.28 s')).toBeDefined();
  expect(within(inspector).getByText('93.8 tokens/s')).toBeDefined();
});

it('paints preparation and first-output boundaries from native request offsets', () => {
  const record = traceRecord(0);
  record.timing.duration_ms = '9000';
  record.request!.generation = {
    timeline: { dispatch_ms: '400', first_output_ms: '720', last_output_ms: '1920', terminal_ms: '2000' },
    ttft_ms: '320', generation_ms: '1280', terminal_ms: '1600', output_tokens_per_second: 93.75,
  };
  renderTrajectory(cacheOf([record]));
  fireEvent.click(screen.getByRole('button', { name: 'Duration' }));
  const span = screen.getByRole('button', { name: 'Inspect Request #0 · historical-model' });
  expect(span.style.getPropertyValue('--trajectory-dispatch')).toBe('20%');
  expect(span.style.getPropertyValue('--trajectory-first-output')).toBe('36%');
});

it('request detail is fetched on demand and renders historical input with its Tool catalog', () => {
  const loads: string[] = [];
  let cache = cacheOf([traceRecord(0)]);
  const ui = renderTrajectory(cache, id => loads.push(id));
  fireEvent.click(screen.getByText('historical-model-0'));
  expect(loads).toEqual(['trace:0']);

  cache = completeTraceDetail(selectTrace(cache, 'trace:0'), 'trace:0', cache.epoch, requestDetail(0));
  ui.rerender(
    <Trajectory cache={cache} loadEarlier={noop} latest={noop} onSelect={noop} onLoadDetail={noop} />,
  );
  const inspector = screen.getByLabelText('Trace record inspector');
  fireEvent.click(within(inspector).getByRole('tab', { name: 'Input' }));
  expect(within(inspector).getByText('You are the historical agent.')).toBeDefined();
  expect(within(inspector).getByText(/Inspect the trajectory/)).toBeDefined();
  fireEvent.click(within(inspector).getByRole('tab', { name: 'Tools' }));
  expect(within(inspector).getByText('bash')).toBeDefined();
  fireEvent.click(within(inspector).getByRole('tab', { name: 'Options' }));
  expect(within(inspector).getByText('temperature')).toBeDefined();
  // The allowlist omission is visible without naming what was omitted.
  expect(within(inspector).getByText(/2 configured request parameters/)).toBeDefined();
});

it('native Tool source renders as code while the original arguments stay inspectable', () => {
  let cache = cacheOf([traceTool(0)]);
  const ui = renderTrajectory(cache);
  fireEvent.click(screen.getByText('ls -la'));
  cache = completeTraceDetail(selectTrace(cache, 'trace:0'), 'trace:0', cache.epoch, toolDetail(0));
  ui.rerender(
    <Trajectory cache={cache} loadEarlier={noop} latest={noop} onSelect={noop} onLoadDetail={noop} />,
  );
  const inspector = screen.getByLabelText('Trace record inspector');
  fireEvent.click(within(inspector).getByRole('tab', { name: 'Source' }));
  expect(within(inspector).getByText(/Highlighted as shell/)).toBeDefined();
  fireEvent.click(within(inspector).getByRole('tab', { name: 'Input' }));
  expect(within(inspector).getByText('Recorded arguments')).toBeDefined();
  fireEvent.click(within(inspector).getByRole('tab', { name: 'Result' }));
  expect(within(inspector).getByText('1.23 s')).toBeDefined();
  fireEvent.click(within(inspector).getByRole('tab', { name: 'Schema' }));
  expect(within(inspector).getByText('Run one command.')).toBeDefined();
});

it('loaded-window search filters rows and says the window is what was searched', () => {
  renderTrajectory(
    cacheOf([
      traceRecord(0, { preview: { text: 'alpha request', truncated: false } }),
      traceRecord(1, { preview: { text: 'beta request', truncated: false } }),
    ]),
  );
  const ledger = screen.getByRole('table', { name: 'Trace ledger' });
  fireEvent.change(screen.getByLabelText('Search loaded Trace'), { target: { value: 'beta' } });
  expect(ledger.querySelectorAll('[data-trace-id]')).toHaveLength(1);
  fireEvent.change(screen.getByLabelText('Search loaded Trace'), { target: { value: 'zzz' } });
  expect(screen.getByText(/Load earlier records to search further back/)).toBeDefined();
});

it('folding a Step hides its rows behind a summary without hiding the next Step', () => {
  renderTrajectory(
    cacheOf([
      traceRecord(0, { preview: { text: 'step one first', truncated: false } }),
      traceRecord(1, { preview: { text: 'step one second', truncated: false } }),
      traceRecord(2, {
        location: { attempt_id: 'attempt-a', step_id: '2' },
        preview: { text: 'step two', truncated: false },
      }),
    ]),
  );
  fireEvent.click(screen.getByLabelText('Fold Step 1'));
  expect(screen.getByText('step one first')).toBeDefined();
  expect(screen.queryByText('step one second')).toBeNull();
  expect(screen.getByText('step two')).toBeDefined();
  fireEvent.click(screen.getByLabelText('Expand Step 1'));
  expect(screen.getByText('step one second')).toBeDefined();
});

it('selection survives a prepend by stable identity and reports removal on rebase', () => {
  const first = traceRecord(10, { preview: { text: 'selected record', truncated: false } });
  let cache = cacheOf([first]);
  const ui = renderTrajectory(cache);
  fireEvent.click(screen.getByText('selected record'));
  expect(screen.getByLabelText('Trace record inspector')).toBeDefined();

  cache = replaceTrace({ records: [traceRecord(9), first], next_cursor: null });
  ui.rerender(
    <Trajectory cache={cache} loadEarlier={noop} latest={noop} onSelect={noop} onLoadDetail={noop} />,
  );
  expect(
    within(screen.getByLabelText('Trace record inspector')).getByText('request-10'),
  ).toBeDefined();

  ui.rerender(
    <Trajectory
      cache={replaceTrace({ records: [traceRecord(9)], next_cursor: null })}
      loadEarlier={noop}
      latest={noop}
      onSelect={noop}
      onLoadDetail={noop}
    />,
  );
  expect(screen.queryByLabelText('Trace record inspector')).toBeNull();
  expect(screen.getByRole('status').textContent).toContain('left the loaded window');
});

it('a long ledger virtualizes to a bounded row window', () => {
  const records = Array.from({ length: 500 }, (_, index) => traceRecord(index));
  const ui = renderTrajectory(cacheOf(records));
  const ledger = screen.getByRole('table', { name: 'Trace ledger' });
  expect(ledger.getAttribute('aria-rowcount')).toBe('500');
  const mounted = ui.container.querySelectorAll('[data-trace-id]').length;
  expect(mounted).toBeGreaterThan(0);
  expect(mounted).toBeLessThan(60);
});

it('tail-follow stops once the reader scrolls upward and resumes at the tail', () => {
  let records = Array.from({ length: 80 }, (_, index) => traceRecord(index + 10));
  const ui = renderTrajectory(cacheOf(records));
  const ledger = screen.getByRole('table', { name: 'Trace ledger' });
  const update = () =>
    ui.rerender(
      <Trajectory
        cache={cacheOf(records)}
        loadEarlier={noop}
        latest={noop}
        onSelect={noop}
        onLoadDetail={noop}
      />,
    );
  // At the tail: a new record follows it.
  ledger.scrollTop = ledger.scrollHeight - 360;
  fireEvent.scroll(ledger);
  records = [...records, traceRecord(90)];
  update();
  expect(ledger.scrollTop).toBe(ledger.scrollHeight);
  // Away from the tail: a new record must not move the reader.
  ledger.scrollTop = 400;
  fireEvent.scroll(ledger);
  records = [...records, traceRecord(91)];
  update();
  expect(ledger.scrollTop).toBe(400);
  // Returning to the tail resumes following.
  ledger.scrollTop = ledger.scrollHeight;
  fireEvent.scroll(ledger);
  records = [...records, traceRecord(92)];
  update();
  expect(ledger.scrollTop).toBe(ledger.scrollHeight);
});

it('a server lifecycle repair settles a selected record retained outside the window', () => {
  const old = traceRecord(1, { state: 'running', timing: { started_at: '2026-09-15T00:00:00Z' } });
  const cache = selectTrace(replaceTrace({ records: [old], next_cursor: 'trace:1' }), old.id);
  const ui = renderTrajectory(cache);
  fireEvent.click(screen.getByText('historical-model-1'));
  const settled = refreshTrace(cache, { records: [traceRecord(100)], next_cursor: null }, [
    {
      id: old.id,
      state: 'completed',
      timing: { ...old.timing, ended_at: '2026-09-15T00:00:02Z', duration_ms: '2500' },
      attachments: [],
      truncated: false,
    },
  ]);
  ui.rerender(
    <Trajectory cache={settled} loadEarlier={noop} latest={noop} onSelect={noop} onLoadDetail={noop} />,
  );
  const inspector = screen.getByLabelText('Trace record inspector');
  expect(within(inspector).getByText('completed')).toBeDefined();
  fireEvent.click(within(inspector).getByRole('tab', { name: 'Timing' }));
  expect(within(inspector).getByText('2.50 s')).toBeDefined();
  expect(screen.getByRole('status').textContent).toContain('retained outside the loaded history');
});

it('the timing overview draws spans only from recorded timing', () => {
  renderTrajectory(
    cacheOf([
      traceRecord(0),
      traceRecord(1, { state: 'running', timing: { started_at: '2026-09-15T00:00:00Z' } }),
    ]),
  );
  const overview = screen.getByLabelText('Timing overview');
  const spans = overview.querySelectorAll('button[data-kind]');
  expect(spans).toHaveLength(2);
  // The unterminated record is a start marker, never a span.
  expect(overview.querySelectorAll('button[data-marker]')).toHaveLength(1);
});

it('timeline and ledger selections refer to the same stable record identity', () => {
  renderTrajectory(cacheOf([traceRecord(0), traceRecord(1)]));
  const overview = screen.getByLabelText('Timing overview');
  fireEvent.click(within(overview).getByLabelText('Inspect Request #0 · historical-model'));
  expect(within(screen.getByLabelText('Trace record inspector')).getByText('request-0')).toBeDefined();
});

it('Chat / Trajectory switching stays on one attachment while live facts change', async () => {
  server = new Server();
  server.snapshots.set('A', { ...snapshot(), trace: { records: [traceRecord(0)] } });
  await server.attached('A');
  localStorage.setItem(
    'rustx-console-view-v2',
    JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', openViews: ['A'] }),
  );
  render(<App client={server.client} workspaceHost={server.workspaceHost} />);
  const attachments = server.requests.filter(item => item.request.method === 'session/attach').length;
  fireEvent.click(screen.getByRole('tab', { name: 'Trajectory' }));
  await act(async () => {
    await server!.update('A', {
      ...snapshot(),
      trace: { records: [traceRecord(0), traceRecord(1)] },
    });
  });
  expect(screen.getByText('historical-model-1')).toBeDefined();
  fireEvent.click(screen.getByRole('tab', { name: 'Chat' }));
  expect(screen.getByLabelText('Canonical conversation')).toBeDefined();
  expect(server.requests.filter(item => item.request.method === 'session/attach')).toHaveLength(attachments);
});

it('Goal chat presentation preserves exact native Tool inspection', () => {
  const record = traceTool(0, { tool: { call_id: 'goal-complete-call', tool_id: 'native.update_goal', name: 'update_goal', started: true, outcome: 'success' }, preview: { text: 'Complete goal', truncated: false } });
  const detail = toolDetail(0);
  detail.tool = { ...detail.tool!, call_id: 'goal-complete-call', tool_id: 'native.update_goal', name: 'update_goal', source: null, arguments: { value: { action: 'complete', expected: { id: 'goal-1', revision: 2 } }, truncated: false }, result: { ...detail.tool!.result!, blocks: [{ type: 'text', text: { text: 'Goal completed exactly', truncated: false } }] } };
  renderTrajectory(completeTraceDetail(cacheOf([record]), record.id, 1, detail));
  fireEvent.click(screen.getByText('Complete goal'));
  const inspector = within(screen.getByLabelText('Trace record inspector'));
  expect(inspector.getByText('native.update_goal')).toBeTruthy();
  expect(inspector.getByText('goal-complete-call')).toBeTruthy();
  fireEvent.click(inspector.getByRole('tab', { name: 'Input' }));
  expect(inspector.getByRole('tree', { name: 'update_goal arguments' }).textContent).toContain('complete');
  fireEvent.click(inspector.getByRole('tab', { name: 'Result' }));
  expect(inspector.getByText('Goal completed exactly')).toBeTruthy();
});

it('one adopted User batch renders every bounded canonical message', () => {
  const record = traceRecord(0, { kind: 'user', request: null, preview: { text: 'Adopted batch', truncated: false } });
  const detail = requestDetail(0, { kind: 'user', request: null, messages: [
    { message_id: 'user-first', role: 'user', source: 'human', blocks: [{ type: 'text', text: { text: 'First adopted message', truncated: false } }], truncated: false },
    { message_id: 'user-second', role: 'user', source: 'human', blocks: [{ type: 'text', text: { text: 'Second adopted message', truncated: false } }], truncated: false },
  ] });
  renderTrajectory(completeTraceDetail(cacheOf([record]), record.id, 1, detail));
  fireEvent.click(screen.getByText('Adopted batch'));
  fireEvent.click(screen.getByRole('tab', { name: 'Content' }));
  expect(screen.getByText('First adopted message')).toBeTruthy();
  expect(screen.getByText('Second adopted message')).toBeTruthy();
});

it.each(['Complete', 'Partial', 'Unavailable'] as const)('shows %s managed output with exact plain-text locator and diagnostic', state => {
  const locator = '/private/rustx-managed-output/example/tasks/<result>.output';
  const diagnostic = `cannot append ${locator}: recorded I/O failure`;
  const detail = toolDetail(0);
  detail.tool!.result!.managed_output = {
    complete: state === 'Complete', available: state !== 'Unavailable',
    locator: state === 'Unavailable' ? null : locator,
    diagnostic: state === 'Complete' ? null : { text: diagnostic, truncated: false },
  };
  let cache = cacheOf([traceTool(0)]);
  cache = completeTraceDetail(selectTrace(cache, 'trace:0'), 'trace:0', cache.epoch, detail);
  renderTrajectory(cache);
  const inspector = within(screen.getByLabelText('Trace record inspector'));
  fireEvent.click(inspector.getByRole('tab', { name: 'Result' }));
  expect(inspector.getByText(state, { exact: true })).toBeDefined();
  if (state !== 'Unavailable') {
    const value = inspector.getByText(locator, { exact: true });
    expect(value.tagName).toBe('DD');
    expect(value.querySelector('a, button')).toBeNull();
  } else {
    expect(inspector.queryByText('Locator', { exact: true })).toBeNull();
  }
  if (state !== 'Complete') expect(inspector.getByText(diagnostic, { exact: true })).toBeDefined();
});
