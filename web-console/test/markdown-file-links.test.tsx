// @vitest-environment jsdom
import { cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { MarkdownText } from './markdown-test-components';
import { FileLinks } from '../src/presentation/markdown/FileLinks';
import { parseFileLink } from '../src/presentation/markdown/file-link';
afterEach(cleanup);
it('parses encoded Unicode paths and line ranges while rejecting non-file destinations', () => {
  expect(parseFileLink('docs/%E8%A7%A3%E8%AA%AC.md#L2-L9')).toEqual({ path: 'docs/解説.md', line: 2 });
  for (const url of ['https://example.com/a.md','//example.com/a.md','javascript:alert(1)','file:///a.md','a.md?x=1','#heading','a.md#L0','a.md#L9-L2','%XX','a%00b']) expect(parseFileLink(url)).toBeUndefined();
});
it('dispatches settled references through the owner while keeping external links and streaming text separate', () => {
  const open = vi.fn(), text = '[解説](docs/%E8%A7%A3%E8%AA%AC.md#L2) and [Web](https://example.com)';
  const tree = (streaming: boolean) => <FileLinks.Provider value={open}><MarkdownText text={text} streaming={streaming}/></FileLinks.Provider>;
  const ui = render(tree(true)); expect(ui.queryByRole('button', { name: '解説' })).toBeNull();
  ui.rerender(tree(false)); fireEvent.click(ui.getByRole('button', { name: '解説' }));
  expect(open).toHaveBeenCalledWith({ path: 'docs/解説.md', line: 2 });
  expect(ui.getByRole('link', { name: 'Web' }).getAttribute('href')).toBe('https://example.com');
});
