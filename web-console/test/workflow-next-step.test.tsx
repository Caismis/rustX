import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { rootBlock } from './workflow-root';
import { nodePresentation } from '../src/app/workflows/document/geometry';
import type { WorkflowDocument } from '../src/app/workflows/document/model';
import type { WorkflowToolSource } from '../../protocol/app-server/v62';
import type { WorkflowScopePort } from '../src/app/workflows/port';
import { NextStep } from '../src/app/workflows/ui/editor/NextStep';
import { ToolCatalogProvider } from '../src/app/workflows/ui/editor/toolCatalog';

afterEach(cleanup);

it('keeps a dangling authored successor removable without hiding its valid sibling', () => {
  const initial: WorkflowDocument = { description: 'Repair an incomplete graph', block: rootBlock() };
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
  const read = { name: 'read', description: 'Read a file. Lines are numbered.', input_schema: { type: 'object', properties: {} }, result: { type: 'text' } } as const;
  let current: WorkflowDocument = { description: 'Read', block: rootBlock() };
  current.block.edges = [];
  function Harness() {
    const [document, setDocument] = useState(current);
    return <NextStep document={document} block={[]} tools={[read]} onSelect={vi.fn()} edit={apply => setDocument(before => { current = apply(before); return current; })} />;
  }
  render(<Harness />);
  // jsdom has no layout: the floating picker stays `visibility: hidden`, so
  // its rows are found by their text.
  fireEvent.click(screen.getByRole('button', { name: /Choose next node/ }));
  // The Nodes tab classifies kinds; Tools lists each builtin Tool.
  expect(screen.getByText('Logic')).toBeTruthy();
  expect(screen.queryByText('Tool')).toBeNull();
  fireEvent.click(screen.getByText('Tools'));
  expect(screen.getByText('Builtin tools')).toBeTruthy();
  fireEvent.change(screen.getByPlaceholderText('Search tools'), { target: { value: 'numbered' } });
  fireEvent.click(screen.getByText('read'));
  const [id, node] = Object.entries(current.block.nodes).find(([, node]) => node.type === 'tool')!;
  expect(node).toEqual({ type: 'tool', selector: { origin: 'builtin', name: 'read' }, arguments: { type: 'object', fields: {} }, result: { type: 'text' } });
  expect(nodePresentation(current, [], id)).toMatchObject({ title: 'read', description: 'Read a file.' });
  expect(current.block.edges).toEqual([{ from: 'start', to: id }]);
});

it('lists each MCP server and Python package of the scope as a provider of its Tools, as Dify', async () => {
  const sources: WorkflowToolSource[] = [
    { source_id: 'github', kind: 'mcp', scope: 'user', state: 'listed', tools: [{ name: 'search_issues', description: 'Search issues.', input_schema: { type: 'object', properties: {} },
      result: { type: 'json', schema: { type: 'object', properties: { total: { type: 'integer' } }, required: ['total'], additionalProperties: false } } }] },
    { source_id: 'python:report-kit', kind: 'python', scope: 'workspace', state: 'preparation_failed', tools: [] },
  ];
  // The scope's sources come first, unlisted; each is then listed alone.
  const toolCatalog = vi.fn(async (source?: string) => ({ sources: source === undefined
    ? sources.map(item => ({ ...item, state: item.state === 'listed' ? 'pending' as const : item.state, tools: [] }))
    : sources.filter(item => item.source_id === source) }));
  const port = { toolCatalog } as unknown as WorkflowScopePort;
  let current: WorkflowDocument = { description: 'Search', block: rootBlock() };
  current.block.edges = [];
  function Harness() {
    const [document, setDocument] = useState(current);
    return <ToolCatalogProvider port={port}><NextStep document={document} block={[]} tools={[]} onSelect={vi.fn()} edit={apply => setDocument(before => { current = apply(before); return current; })} /></ToolCatalogProvider>;
  }
  render(<Harness />);
  fireEvent.click(screen.getByRole('button', { name: /Choose next node/ }));
  // Listing connects to every source, so it waits for the Tools tab.
  expect(toolCatalog).not.toHaveBeenCalled();
  fireEvent.click(screen.getByText('Tools'));
  expect(await screen.findByText('github')).toBeTruthy();
  await screen.findByText('1');
  expect(toolCatalog.mock.calls).toEqual([[], ['github']]);
  expect(screen.getByText('report-kit')).toBeTruthy();
  expect(screen.getByText('Environment not prepared')).toBeTruthy();
  expect(screen.getByText('Workspace')).toBeTruthy();
  expect(screen.queryByText('search_issues')).toBeNull();
  fireEvent.click(screen.getByText('github'));
  fireEvent.click(screen.getByText('search_issues'));
  const [id, node] = Object.entries(current.block.nodes).find(([, node]) => node.type === 'tool')!;
  expect(node).toEqual({ type: 'tool', selector: { origin: 'source', source_id: 'github', name: 'search_issues' }, arguments: { type: 'object', fields: {} }, result: sources[0].tools[0].result });
  expect(nodePresentation(current, [], id)).toMatchObject({ title: 'search_issues', description: 'Search issues.' });
});
