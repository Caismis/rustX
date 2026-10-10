import { act, cleanup, fireEvent, render, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { ForegroundToolExecution, SessionFileReference, ToolExecutionResult } from '../../protocol/app-server/v44';
import { presentRow, presentedDeliveries } from '../src/bindings/present';
import { Tool } from '../src/app/agent/Tool';
import { ToolDeliveries } from '../src/app/components/Artifact';
import { PreviewContext } from '../src/app/components/ArtifactPreview';
import { localeController } from '../src/locale/controller';

afterEach(() => { cleanup(); act(() => localeController.setLocale('en')); });
const file = (name: string, description: string | null = null): SessionFileReference =>
  ({ scope: { conversation_id: 'original', device: '1', inode: '2' }, path: `out/${name}`, name, description, mime_type: 'text/plain' });
const ARGS = JSON.stringify({ files: [{ path: 'out/报告 file.md' }, { path: 'out/data set.csv' }] });
const result = (status: ToolExecutionResult['status'], deliveries: SessionFileReference[] = []): ToolExecutionResult =>
  ({ status, duration_ms: 1, content: [{ type: 'text', text: 'Declared 1 deliverable(s)' }], deliveries });
const tool = (state: ForegroundToolExecution['state']): ForegroundToolExecution =>
  ({ call_id: 'call-present', message_id: 'assistant-1', block_index: 0, tool_id: 'tool-present', name: 'present', state });

it('maps only the native lifecycle to Harness present phases', () => {
  expect(presentRow(tool({ type: 'assembled', arguments: ARGS })).phase).toBe('preparing');
  expect(presentRow(tool({ type: 'running', arguments: ARGS })).phase).toBe('running');
  expect(presentRow(tool({ type: 'settled', arguments: ARGS, result: result({ type: 'success' }) })).phase).toBe('ok');
  expect(presentRow(tool({ type: 'settled', arguments: ARGS, result: result({ type: 'cancelled', reason: 'interrupt' } as never) })).phase).toBe('stopped');
  for (const status of [{ type: 'failed', error: 'missing file' }, { type: 'denied', reason: 'policy' }] as const) {
    const row = presentRow(tool({ type: 'settled', arguments: ARGS, result: result(status) }));
    expect(row.phase).toBe('error');
    expect(row.details).toContain('error' in status ? status.error : status.reason);
  }
  expect(presentRow(tool({ type: 'assembled', arguments: ARGS })).paths).toBe('out/报告 file.md, out/data set.csv');
  expect(presentRow(tool({ type: 'assembled', arguments: '{"files":[{"pa' })).paths).toBe('{"files":[{"pa');
});

it('the call row never renders delivery cards, even for a settled success carrying deliveries', () => {
  const ui = render(<Tool tool={tool({ type: 'settled', arguments: ARGS, result: result({ type: 'success' }, [file('报告 file.md')]) })}/>);
  expect(ui.container.querySelector('[data-delivery-card]')).toBeNull();
  const row = ui.container.querySelector('[data-tool="present"]')!;
  expect(row.getAttribute('data-present-phase')).toBe('ok');
  expect(row.textContent).toContain('Present files');
  expect(row.textContent).toContain('Delivered');
  const disclosure = within(row as HTMLElement).getByRole('button');
  expect(disclosure.getAttribute('aria-expanded')).toBe('false');
  fireEvent.keyDown(disclosure, { key: 'Enter' });
  expect(disclosure.getAttribute('aria-expanded')).toBe('true');
  expect(row.textContent).toContain('Declared 1 deliverable(s)');
  act(() => localeController.setLocale('zh'));
  expect(row.textContent).toContain('交付文件');
  expect(row.textContent).toContain('已交付');
  ui.rerender(<Tool tool={tool({ type: 'assembled', arguments: ARGS })}/>);
  expect(ui.container.querySelector('[data-tool="present"]')!.getAttribute('aria-label')).toBe('准备交付');
});

it('committed cards: canonical order, four-card summary, separate Preview and Download, localized', () => {
  const files = ['报告 file.md', 'b.txt', 'c.rs', 'd.png', 'e.bin', 'f.txt', 'g.csv'].map((name, index) => file(name, index === 0 ? 'Final report (draft)' : null));
  expect(presentedDeliveries('m', result({ type: 'failed', error: 'x' }, files))).toEqual([]);
  expect(presentedDeliveries('m', { ...result({ type: 'success' }), content: [{ type: 'json', value: { deliveries: files } }] })).toEqual([]);
  const openPreview = vi.fn(), download = vi.fn();
  const ui = render(<PreviewContext.Provider value={{ openPreview, download }}><ToolDeliveries messageId="canonical-tool" result={result({ type: 'success' }, files)}/></PreviewContext.Provider>);
  const names = () => [...ui.container.querySelectorAll('[data-presented-name]')].map(node => node.textContent);
  expect(names()).toEqual(files.slice(0, 4).map(item => item.name));
  // Harness strips a trailing parenthetical; the extension stands in for an absent description.
  expect(ui.container.querySelector('[data-presented-description]')!.textContent).toContain('Final report');
  expect(ui.container.querySelector('[data-presented-description]')!.textContent).not.toContain('(draft)');
  const toggle = ui.getByRole('button', { name: 'Show all 7 delivered files' });
  expect(toggle.getAttribute('aria-expanded')).toBe('false');
  fireEvent.click(toggle);
  expect(names()).toEqual(files.map(item => item.name));
  fireEvent.click(ui.getByRole('button', { name: 'Download b.txt' }));
  expect(download).toHaveBeenCalledWith(expect.objectContaining({ source: { kind: 'session_file', messageId: 'canonical-tool', index: 1, file: files[1] }, name: 'b.txt' }));
  expect(openPreview).not.toHaveBeenCalled();
  fireEvent.click(ui.getByRole('button', { name: 'Preview 报告 file.md in sidebar' }));
  expect(openPreview).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ source: { kind: 'session_file', messageId: 'canonical-tool', index: 0, file: files[0] } }));
  act(() => localeController.setLocale('zh'));
  expect(ui.getByRole('button', { name: '在侧边栏预览 报告 file.md' })).toBeTruthy();
  expect(ui.getByRole('button', { name: '收起交付文件列表' })).toBeTruthy();
});
