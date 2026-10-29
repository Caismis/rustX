import { expect, it } from 'vitest';
import { exampleSource } from './workflow-fixtures';
import type { WorkflowDocument } from '../src/app/workflows/document/model';
import { entryPresentation, setEntryPosition } from '../src/app/workflows/document/geometry';
import { fieldValue, setField, type FieldAddress } from '../src/app/workflows/document/fields';

const document = exampleSource('parallel_review').document as unknown as WorkflowDocument;
const title: FieldAddress = { block: [], path: ['title'], role: 'value', domain: { kind: 'entry_presentation' } };

it('edits the Start node title and description as presentation, keeping its position', () => {
  expect(fieldValue(document, title)).toEqual({ value: '' });
  const named = setField(document, title, 'User input');
  expect(entryPresentation(named, [])).toEqual({ block: [], position: { x: 32, y: 80 }, title: 'User input' });
  const moved = setEntryPosition(named, [], { x: 100, y: 120 });
  expect(entryPresentation(moved, [])).toEqual({ block: [], position: { x: 100, y: 120 }, title: 'User input' });
  const described = setField(moved, { ...title, path: ['description'] }, 'Where the user uploads files');
  expect(entryPresentation(described, [])?.description).toBe('Where the user uploads files');
  // The executable definition never changes.
  expect(described.block).toBe(document.block);
});
