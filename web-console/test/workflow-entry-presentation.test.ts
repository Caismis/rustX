import { expect, it } from 'vitest';
import { emptyBlock } from '../src/app/workflows/document/commands';
import { nodePresentation, setNodePresentation } from '../src/app/workflows/document/geometry';
import { fieldValue, nodeField, setField } from '../src/app/workflows/document/fields';
import type { WorkflowDocument } from '../src/app/workflows/document/model';

it('edits real Start node presentation without changing the executable graph', () => {
  const document: WorkflowDocument = { description: 'Start presentation', block: emptyBlock() };
  const title = { ...nodeField([], 'start', ['title']), domain: { kind: 'node_presentation' as const } };
  expect(fieldValue(document, title)).toEqual({ value: 'start' });
  const named = setField(document, title, 'User input');
  const moved = setNodePresentation(named, [], 'start', { position: { x: 100, y: 120 } });
  const described = setField(moved, { ...title, path: ['description'] }, 'Where the user uploads files');
  expect(nodePresentation(described, [], 'start')).toMatchObject({ address: { block: [], node: 'start' }, position: { x: 100, y: 120 }, title: 'User input', description: 'Where the user uploads files' });
  expect(described.block).toBe(document.block);
});
