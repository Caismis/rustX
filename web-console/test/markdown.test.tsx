import { afterEach, expect, it } from 'vitest';
import { cleanup, render } from '@testing-library/react';
import { MarkdownText } from '../src/presentation/markdown/MarkdownText';
afterEach(cleanup);
const document = '# Heading\n\nText **bold** and *emphasis* [link](https://example.com) `inline`.\n\n> Quote\n\n1. First\n2. Second\n\n- Other\n\n| A | B |\n| - | - |\n| x | y |\n\n```rust\nfn main() {}\n```\n\nMath $x^2$ and \\(y\\).\n\n$$\na+b\n$$\n\nBad $\\notacommand$';
it('renders rich semantic content and math, with safe malformed fallback', () => {
  const { container } = render(<MarkdownText text={document} />);
  for (const tag of ['h1', 'strong', 'em', 'a', 'code', 'blockquote', 'ol', 'ul', 'table', 'pre', '.katex']) expect(container.querySelector(tag)).not.toBeNull();
  expect(container.querySelector('a')?.rel).toBe('noopener noreferrer');
});
for (const step of [1, 3, 11]) it(`settles awkward chunks into identical rich output (${step})`, () => {
  const live = render(<MarkdownText text="" streaming />);
  for (let end = step; end < document.length; end += step) live.rerender(<MarkdownText text={document.slice(0, end)} streaming />);
  live.rerender(<MarkdownText text={document} />);
  const once = render(<MarkdownText text={document} />);
  expect(live.container.innerHTML).toBe(once.container.innerHTML);
});
it('never executes HTML, unsafe URLs, local images or trusted TeX commands', () => {
  const { container } = render(<MarkdownText text={'<script>alert(1)</script>\n\n[x](javascript:alert%281%29) ![local](/etc/passwd)\n\n$\\href{javascript:alert(1)}{bad}$'} />);
  expect(container.querySelector('script,img,iframe')).toBeNull();
  expect(container.querySelector('[href^="javascript:"]')).toBeNull();
  expect(container.textContent).toContain('local');
});

it('retains one current open-code node, with no historical AST chain per chunk', async () => {
  const { IncrementalMarkdownParser } = await import('../src/presentation/markdown/incremental');
  const { parseGfm } = await import('../src/presentation/markdown/parse');
  const parser = new IncrementalMarkdownParser(parseGfm);
  let source = '```rust\n';
  for (let line = 0; line < 1200; line++) { source += `let v${line} = ${line};\n`; parser.update(source); }
  const seen = new Set<object>(); let codeNodes = 0; let retainedStringUnits = 0;
  function visit(value: unknown) {
    if (typeof value === 'string') { retainedStringUnits += value.length; return; }
    if (!value || typeof value !== 'object' || seen.has(value)) return;
    seen.add(value);
    if ('type' in value && value.type === 'code') codeNodes++;
    for (const child of Object.values(value)) visit(child);
  }
  visit(parser);
  expect(codeNodes).toBe(1);
  expect(retainedStringUnits).toBeLessThan(source.length * 4);
  expect(seen.size).toBeLessThan(30);
  const closed = parser.update(`${source}\`\`\`\n\nafter`);
  expect([...closed.frozen, ...closed.tail].find(block => block.node.type === 'code')?.node)
    .toEqual(parseGfm(`${source}\`\`\`\n\nafter`).children[0]);
});
