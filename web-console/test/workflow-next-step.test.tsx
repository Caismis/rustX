import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { emptyBlock } from '../src/app/workflows/document/commands';
import { nodePresentation } from '../src/app/workflows/document/geometry';
import type { WorkflowDocument } from '../src/app/workflows/document/model';
import { NextStep } from '../src/app/workflows/ui/editor/NextStep';

afterEach(cleanup);

it('keeps a dangling authored successor removable without hiding its valid sibling', () => {
  const initial: WorkflowDocument = { description: 'Repair an incomplete graph', block: emptyBlock() };
  initial.block.edges!.push({ from: 'start', to: 'missing' });
  let current = initial;
  function Harness() {
    const [document, setDocument] = useState(initial);
    return <NextStep document={document} block={[]} tools={[]} onSelect={vi.fn()} edit={apply => setDocument(before => {
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

it('adds a node per builtin Tool from the Tools tab, titled and described by that Tool, as Dify', () => {
  const read = { name: 'read', description: 'Read a file. Lines are numbered.', input_schema: { type: 'object', properties: {} } };
  let current: WorkflowDocument = { description: 'Read', block: emptyBlock() };
  current.block.edges = [];
  function Harness() {
    const [document, setDocument] = useState(current);
    return <NextStep document={document} block={[]} tools={[read]} onSelect={vi.fn()} edit={apply => setDocument(before => { current = apply(before); return current; })} />;
  }
  render(<Harness />);
  // jsdom has no layout: the floating picker stays `visibility: hidden`, so
  // its rows are found by their text.
  fireEvent.click(screen.getByRole('button', { name: /Choose next node/ }));
  // The Nodes tab classifies kinds; Tools lists each Tool, then source Tools.
  expect(screen.getByText('Logic')).toBeTruthy();
  expect(screen.queryByText('Tool')).toBeNull();
  fireEvent.click(screen.getByText('Tools'));
  expect(screen.getByText('Builtin tools')).toBeTruthy();
  expect(screen.getByText('MCP or Python tool')).toBeTruthy();
  fireEvent.change(screen.getByPlaceholderText('Search tools'), { target: { value: 'numbered' } });
  fireEvent.click(screen.getByText('read'));
  const [id, node] = Object.entries(current.block.nodes).find(([, node]) => node.type === 'tool')!;
  expect(node).toEqual({ type: 'tool', selector: { origin: 'builtin', name: 'read' }, arguments: { type: 'object', fields: {} }, result: { type: 'text', part: 0 } });
  expect(nodePresentation(current, [], id)).toMatchObject({ title: 'read', description: 'Read a file.' });
  expect(current.block.edges).toEqual([{ from: 'start', to: id }]);
});
