import { act, cleanup, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';

afterEach(() => { cleanup(); vi.doUnmock('@shikijs/langs/rust'); vi.resetModules(); });

for (const loadBeforeSettlement of [true, false]) it(`canonical Rust settlement when registration is ${loadBeforeSettlement ? 'before' : 'after'} settlement`, async () => {
  vi.resetModules();
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  // Hold only the real Rust grammar import; production loading/registration and
  // useSyncExternalStore notifications remain real. No timer/order assumption.
  vi.doMock('@shikijs/langs/rust', async () => {
    await gate;
    return vi.importActual('@shikijs/langs/rust');
  });
  const { CodeBlock } = await import('../src/presentation/markdown/CodeBlock');
  const { grammarLoadCount, subscribeGrammarLoaded } = await import('../src/presentation/markdown/highlight');
  expect(grammarLoadCount()).toBe(0);
  const props = { code: 'fn main() {\n    println!("hello");\n}\n', lang: 'rust', copyLabel: 'Copy', copiedLabel: 'Copied' };
  const live = render(<CodeBlock {...props} streaming />);
  expect(live.container.querySelector('pre.shiki')).toBeNull();
  const registered = new Promise<void>(resolve => {
    const unsubscribe = subscribeGrammarLoaded(() => { unsubscribe(); resolve(); });
  });
  if (!loadBeforeSettlement) live.rerender(<CodeBlock {...props} />);
  await act(async () => { release(); await registered; });
  expect(grammarLoadCount()).toBe(1);
  expect(live.container.querySelector('pre.shiki')).not.toBeNull();
  // Same mounted instance; registration has actually re-rendered it before settle.
  live.rerender(<CodeBlock {...props} />);
  const cold = render(<CodeBlock {...props} />);
  expect(live.container.innerHTML).toBe(cold.container.innerHTML);
  const ready = render(<CodeBlock {...props} streaming />);
  expect(ready.container.querySelector('pre.shiki')).not.toBeNull();
  ready.rerender(<CodeBlock {...props} />);
  expect(ready.container.innerHTML).toBe(cold.container.innerHTML);
});
