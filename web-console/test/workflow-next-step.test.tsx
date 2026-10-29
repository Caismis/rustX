import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { emptyBlock } from '../src/app/workflows/document/commands';
import type { WorkflowDocument } from '../src/app/workflows/document/model';
import { NextStep } from '../src/app/workflows/ui/editor/NextStep';

afterEach(cleanup);

it('keeps a dangling authored successor removable without hiding its valid sibling', () => {
  const initial: WorkflowDocument = { description: 'Repair an incomplete graph', block: emptyBlock() };
  initial.block.edges!.push({ from: 'start', to: 'missing' });
  let current = initial;
  function Harness() {
    const [document, setDocument] = useState(initial);
    return <NextStep document={document} block={[]} onSelect={vi.fn()} edit={apply => setDocument(before => {
      current = apply(before); return current;
    })} />;
  }
  render(<Harness />);
  expect(screen.getByRole('button', { name: 'done' })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'missing' })).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Disconnect missing' }));
  expect(screen.queryByRole('button', { name: 'missing' })).toBeNull();
  expect(screen.getByRole('button', { name: 'done' })).toBeTruthy();
  expect(current.block.edges).toEqual([{ from: 'start', to: 'done' }]);
});
