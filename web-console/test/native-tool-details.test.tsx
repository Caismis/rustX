import { cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { Tool } from '../src/app/agent/Tool';
import { ToolInspectionContext } from '../src/app/agent/tool-inspection';
import { translator } from '../src/locale/translation';
import { recordedToolDetails } from '../src/bindings/recorded-tool-details';
import { nativeToolCases, nativeTool, jobResult, goalResult } from './native-tool-cases';
import { TrajectoryInspector } from '../src/app/trajectory/TrajectoryInspector';
import { traceTool, toolDetail } from './trace-fixture';

afterEach(cleanup);
const tx = translator('en');
it('covers every fixed native registration, optional Todo and the discovered Workflow family', () => {
  const names = new Set(['ask_user', 'read_image', 'present', 'subagent', 'todo']);
  for (const module of ['read', 'write', 'edit', 'glob', 'grep', 'bash']) {
    const source = readFileSync(resolve(process.cwd(), `../src/tools/native/${module}/mod.rs`), 'utf8');
    const name = source.match(/const NAME: &str = "([^"]+)"/u)?.[1];
    expect(name, module).toBeDefined(); names.add(name!);
  }
  for (const module of ['jobs', 'agents', 'goal']) {
    const source = readFileSync(resolve(process.cwd(), `../src/tools/native/${module}.rs`), 'utf8');
    const list = source.match(/const NAMES:[\s\S]*?= \[([\s\S]*?)\];/u)?.[1];
    expect(list, module).toBeDefined();
    for (const match of list!.matchAll(/"([^"]+)"/gu)) names.add(match[1]);
  }
  expect(new Set(nativeToolCases.map(tool => tool.name))).toEqual(new Set([...names, 'review']));
});
it.each(nativeToolCases.map(tool => [tool.name, tool] as const))('%s has a keyboard disclosure and one native Inspect action without an execution-envelope dump', async (_name, tool) => {
  const inspect = vi.fn(async () => {});
  const ui = render(<ToolInspectionContext value={inspect}><Tool tool={tool}/></ToolInspectionContext>);
  const toggle = ui.getByRole('button', { expanded: false });
  fireEvent.keyDown(toggle, { key: 'Enter' });
  expect(toggle.getAttribute('aria-expanded')).toBe('true');
  expect(ui.container.textContent).not.toMatch(/Full task record|Recorded task result|Execution details|"committed_result_deferred"|"tool_id"/u);
  fireEvent.click(ui.getByRole('button', { name: 'Inspect' }));
  expect(inspect).toHaveBeenCalledExactlyOnceWith({ occurrence: tool.occurrence, call_id: tool.call_id, tool_id: tool.tool_id });
  fireEvent.keyDown(toggle, { key: ' ' });
  expect(toggle.getAttribute('aria-expanded')).toBe('false');
});
it('uses the committed body without replacing lifecycle, and keeps unknown payloads inspectable', () => {
  const tool = nativeToolCases.find(tool => tool.name === 'job_wait')!;
  const committed = { ...tool, committed_result: { status: { type: 'success' as const }, duration_ms: '1', content: [{ type: 'json' as const, value: { ...jobResult, result: { content: [{ type: 'text', text: 'Committed native output' }] } } }] } };
  const ui = render(<Tool tool={committed}/>);
  fireEvent.click(ui.getByRole('button', { expanded: false }));
  expect(ui.getByText('Committed native output')).toBeTruthy();
  expect(ui.queryByText(/23:03:13/)).toBeNull();
  ui.rerender(<Tool tool={nativeTool('tool-job_wait', 'job_wait', { job_id: 'unknown' }, [{ type: 'json', value: { unsupported: 'Original payload' } }])}/>);
  expect(ui.container.textContent).toContain('Original payload');
  expect(ui.container.querySelector('[data-tool-details]')).toBeNull();
});
it.each(['read', 'write', 'edit', 'glob', 'grep'])('%s failures use diagnostic output instead of a content/result card', name => {
  const tool = nativeToolCases.find(tool => tool.name === name)!;
  const ui = render(<Tool tool={{ ...tool, state: { type: 'settled', arguments: tool.state.arguments, result: { status: { type: 'failed', error: 'Native access refused' }, duration_ms: '0' } } }}/>);
  fireEvent.click(ui.getByRole('button', { expanded: false }));
  expect(ui.container.querySelector('[data-read], [data-search], [data-diff]')).toBeNull();
  expect(ui.container.textContent).toContain('Native access refused');
});
it('goal details state recorded phase, exact round count and blocked reason, including an empty goal', () => {
  const details = recordedToolDetails('native.get_goal', [{ type: 'json', value: goalResult }], tx)!;
  expect(details.items[0]).toMatchObject({ title: goalResult.objective, fields: [{ label: 'State', value: 'Blocked' }, { label: 'Rounds', value: '3 / 10' }, { label: 'Reason', value: goalResult.blocked_reason }] });
  expect(recordedToolDetails('native.get_goal', [{ type: 'json', value: null }], tx)).toMatchObject({ items: [], empty: 'No current goal' });
  expect(recordedToolDetails('mcp.get_goal', [{ type: 'json', value: goalResult }], tx)).toBeUndefined();
});
it('answered questions use committed answers and offer native Inspect without exposing the transport record', () => {
  const original = nativeToolCases.find(tool => tool.name === 'ask_user')!;
  const committed = { ...original, committed_result: { status: { type: 'success' as const }, duration_ms: '1', content: [{ type: 'json' as const, value: { cancelled: false, answers: [{ kind: 'single', question_index: 0, answer: 'Committed answer' }] } }] } };
  const ui = render(<Tool tool={committed}/>);
  fireEvent.click(ui.getByRole('button', { expanded: false }));
  expect(ui.getByText('Committed answer')).toBeTruthy();
  expect(ui.queryByText('标准')).toBeNull();
  expect(ui.container.textContent).not.toContain('question_index');
});
it('Todo renders the recorded post-call status and dependency edges, including tombstones', () => {
  const details = recordedToolDetails('tool-todo', [{ type: 'text', text: 'Updated task' }, { type: 'json', value: { tasks: [
    { id: 1, subject: 'Retired task', status: 'deleted' }, { id: 2, subject: 'Follow up', status: 'pending', blocked_by: [1], owner: 'explore' },
  ] } }], tx)!;
  expect(details.items[0]).toMatchObject({ title: 'Retired task', status: { value: 'deleted', label: 'Deleted' } });
  expect(details.items[1]).toMatchObject({ status: { value: 'pending', label: 'Pending' }, fields: [{ label: 'Owner', value: 'explore' }, { label: 'Depends on', value: '#1' }] });
});
it('Trace overview shows the same Job output; Result retains the original JSON without a second disclosure', () => {
  const record = traceTool(1); record.tool = { ...record.tool!, name: 'job_wait', tool_id: 'tool-job_wait' };
  const detail = toolDetail(1); detail.tool = { ...detail.tool!, name: 'job_wait', tool_id: 'tool-job_wait', source: null, result: { ...detail.tool!.result!, blocks: [{ type: 'json', value: { value: jobResult, truncated: false } }] } };
  const props = { item: { type: 'RecordRow' as const, record, display_key: record.id, owner_record_id: record.id, facet: 'overview' as const, label: '', preview: '' }, onFacet: vi.fn(), onLoadDetail: vi.fn(), onClose: vi.fn(), records: [record], executions: new Map(), requestNumbers: new Map(), completeHistory: true, onOpen: vi.fn(), detail };
  const ui = render(<TrajectoryInspector {...props} facet="overview"/>);
  expect(ui.container.querySelector('[data-tool-details]')).toBeTruthy();
  expect(ui.container.textContent).toContain('23:03:43');
  expect(ui.queryByText('Full task record')).toBeNull();
  ui.rerender(<TrajectoryInspector {...props} facet="output"/>);
  expect(ui.getByRole('tree', { name: 'Result JSON' })).toBeTruthy();
  expect(ui.container.querySelector('[data-tool-details]')).toBeNull();
});
