import { cleanup, fireEvent, render, within } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { PartsTree, type PartsTreeLabels } from '../src/app/components/documents/geometry/PartsTree';
import type { Part } from '../src/app/components/documents/geometry/structure';
import { en, zh } from '../src/locale/dictionaries/geometry';

afterEach(cleanup);
const labels: PartsTreeLabels = { parts: 'Parts', part: 'Part', expand: 'Expand', collapse: 'Collapse', visible: 'Visible', hiddenParent: 'Hidden by an ancestor', hide: 'Hide part', show: 'Show part', searchParts: 'Search parts', clearPartsSearch: 'Clear parts search', noPartsMatch: 'No matching parts' };
const parts: Part[] = [
  { id: '0', name: 'Assembly', depth: 0, kind: 'group' },
  { id: '0.1', name: 'Branch one', depth: 1, kind: 'group' },
  { id: '0.1.0', name: 'Needle one', depth: 2, kind: 'mesh' },
  { id: '0.10', name: 'Branch ten', depth: 1, kind: 'group' },
  { id: '0.10.0', name: 'Needle ten 涡轮', depth: 2, kind: 'mesh' },
];
function setup(options: { expanded?: string[]; hidden?: string[]; selected?: string; parts?: Part[] } = {}) {
  const selected = vi.fn(), visibility = vi.fn(), expansion = vi.fn();
  function Harness() {
    const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set(options.expanded));
    return <PartsTree parts={options.parts ?? parts} labels={labels} selected={options.selected} hidden={options.hidden} expanded={expanded} onExpandedChange={next => { expansion(next); setExpanded(next); }} onSelect={selected} onVisibility={visibility}/>;
  }
  const ui = render(<Harness/>), search = ui.getByRole('searchbox', { name: labels.searchParts });
  return { ...ui, search, selected, visibility, expansion, query: (value: string) => fireEvent.change(search, { target: { value } }) };
}

it('reveals matching paths with whole-ID-segment ancestors, including English and Chinese names', () => {
  const ui = setup();
  ui.query('NEEDLE TEN');
  const tree = within(ui.getByRole('list', { name: 'Parts' }));
  expect(tree.getByRole('button', { name: 'Assembly' })).toBeTruthy();
  expect(tree.getByRole('button', { name: 'Branch ten' })).toBeTruthy();
  expect(tree.getByRole('button', { name: 'Needle ten 涡轮' })).toBeTruthy();
  expect(tree.queryByRole('button', { name: 'Branch one' })).toBeNull();
  expect(tree.getByRole('button', { name: 'Collapse: Assembly' })).toHaveProperty('disabled', true);
  ui.query('涡轮'); expect(tree.getByRole('button', { name: 'Needle ten 涡轮' })).toBeTruthy();
  expect(ui.selected).not.toHaveBeenCalled(); expect(ui.visibility).not.toHaveBeenCalled(); expect(ui.expansion).not.toHaveBeenCalled();
});
it('matches assembly names without adding unrelated descendants to the filtered result', () => {
  const ui = setup(); ui.query('BRANCH TEN');
  expect(ui.getByRole('button', { name: 'Branch ten' })).toBeTruthy();
  expect(ui.queryByRole('button', { name: 'Needle ten 涡轮' })).toBeNull();
  expect(ui.queryByRole('button', { name: 'Branch one' })).toBeNull();
});
it('searches the current localized fallback name without changing unnamed source metadata', () => {
  const unnamed: Part[] = [parts[0], { id: '0.0', name: '', depth: 1, kind: 'mesh' }];
  const props = { parts: unnamed, expanded: new Set<string>(), onExpandedChange: vi.fn(), onSelect: vi.fn(), onVisibility: vi.fn() };
  const ui = render(<PartsTree {...props} labels={en}/>);
  fireEvent.change(ui.getByRole('searchbox'), { target: { value: `${en.part.toUpperCase()} 2` } });
  expect(ui.getByRole('button', { name: `${en.part} 2` })).toBeTruthy();
  ui.rerender(<PartsTree {...props} labels={zh}/>);
  expect(ui.getByRole('status').textContent).toBe(zh.noPartsMatch);
  fireEvent.change(ui.getByRole('searchbox', { name: zh.searchParts }), { target: { value: `${zh.part} 2` } });
  expect(ui.getByRole('button', { name: `${zh.part} 2` })).toBeTruthy(); expect(unnamed[1].name).toBe('');
  expect(props.onSelect).not.toHaveBeenCalled(); expect(props.onVisibility).not.toHaveBeenCalled(); expect(props.onExpandedChange).not.toHaveBeenCalled();
});
it('restores exact manual expansion after clearing a query without writing forced expansion', () => {
  const ui = setup();
  fireEvent.click(ui.getByRole('button', { name: 'Expand: Assembly' }));
  fireEvent.click(ui.getByRole('button', { name: 'Expand: Branch one' }));
  expect(ui.expansion).toHaveBeenCalledTimes(2);
  ui.query('needle ten');
  expect(ui.getByRole('button', { name: 'Needle ten 涡轮' })).toBeTruthy();
  expect(ui.expansion).toHaveBeenCalledTimes(2);
  fireEvent.click(ui.getByRole('button', { name: labels.clearPartsSearch }));
  expect(document.activeElement).toBe(ui.search);
  expect(ui.getByRole('button', { name: 'Collapse: Assembly' })).toHaveProperty('disabled', false);
  expect(ui.getByRole('button', { name: 'Needle one' })).toBeTruthy();
  expect(ui.getByRole('button', { name: 'Expand: Branch ten' })).toBeTruthy();
  expect(ui.queryByRole('button', { name: 'Needle ten 涡轮' })).toBeNull();
  expect(ui.expansion.mock.calls.at(-1)?.[0]).toEqual(new Set(['0', '0.1']));
});
it('preserves inherited hidden state and selection while forwarding only explicit row actions', () => {
  const ui = setup({ hidden: ['0.1'], selected: '0.10.0' });
  ui.query('needle ten');
  const ten = ui.getByRole('checkbox', { name: 'Visible: Needle ten 涡轮' });
  expect(ten.hasAttribute('aria-description')).toBe(false);
  expect(ui.getByRole('button', { name: 'Needle ten 涡轮' }).getAttribute('aria-pressed')).toBe('true');
  ui.query('needle one');
  const one = ui.getByRole('checkbox', { name: 'Visible: Needle one' });
  expect(one.getAttribute('aria-description')).toBe(labels.hiddenParent); expect(one.getAttribute('aria-checked')).toBe('true');
  expect(one.closest('[data-hidden]')).toBeTruthy();
  expect(ui.selected).not.toHaveBeenCalled(); expect(ui.visibility).not.toHaveBeenCalled();
  fireEvent.click(one); expect(ui.visibility).toHaveBeenCalledExactlyOnceWith('0.1.0', false);
  expect(one.getAttribute('aria-checked')).toBe('true');
  fireEvent.click(ui.getByRole('button', { name: 'Needle one' })); expect(ui.selected).toHaveBeenCalledExactlyOnceWith('0.1.0');
  ui.query('needle ten'); fireEvent.click(ui.getByRole('button', { name: 'Needle ten 涡轮' })); expect(ui.selected).toHaveBeenLastCalledWith(undefined);
  expect(ui.expansion).not.toHaveBeenCalled();
});
it('bounds programmatic queries and reports no matches without mutating scene callbacks', () => {
  const ui = setup(); ui.query('x'.repeat(1024));
  expect(ui.search).toHaveProperty('value', 'x'.repeat(256)); expect(ui.search).toHaveProperty('maxLength', 256);
  expect(ui.getByRole('status').textContent).toBe(labels.noPartsMatch);
  expect(within(ui.getByRole('list', { name: 'Parts' })).queryAllByRole('listitem')).toHaveLength(0);
  expect(ui.selected).not.toHaveBeenCalled(); expect(ui.visibility).not.toHaveBeenCalled(); expect(ui.expansion).not.toHaveBeenCalled();
  ui.query('   '); expect(ui.queryByRole('status')).toBeNull(); expect(ui.getByRole('button', { name: 'Assembly' })).toBeTruthy();
});
it('keeps search transient across keyed panel/copy mounts', () => {
  const props = { parts, labels, expanded: new Set(['0']), onExpandedChange: vi.fn(), onSelect: vi.fn(), onVisibility: vi.fn() };
  const ui = render(<PartsTree key="first-copy" {...props}/>);
  fireEvent.change(ui.getByRole('searchbox'), { target: { value: 'Needle one' } });
  ui.rerender(<PartsTree key="second-copy" {...props}/>);
  expect(ui.getByRole('searchbox')).toHaveProperty('value', ''); expect(ui.queryByRole('button', { name: 'Needle one' })).toBeNull();
  expect(ui.getByRole('button', { name: 'Expand: Branch one' })).toBeTruthy(); expect(props.onExpandedChange).not.toHaveBeenCalled();
});
it('searches the entire admitted 512-node metadata envelope without truncating later names', () => {
  const admitted: Part[] = [{ id: '0', name: 'Assembly', depth: 0, kind: 'group' }];
  for (let index = 0; index < 255; index++) admitted.push({ id: `0.${index}`, name: `Group ${index}`, depth: 1, kind: 'group' }, { id: `0.${index}.0`, name: `Part ${index}`, depth: 2, kind: 'mesh' });
  admitted.push({ id: '0.255', name: 'Final leaf', depth: 1, kind: 'mesh' });
  expect(admitted).toHaveLength(512); const ui = setup({ parts: admitted }); ui.query('final LEAF');
  expect(ui.getByRole('button', { name: 'Final leaf' })).toBeTruthy();
  expect(within(ui.getByRole('list', { name: 'Parts' })).getAllByRole('listitem')).toHaveLength(2);
});
