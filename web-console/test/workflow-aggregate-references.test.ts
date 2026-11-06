import { describe, expect, it } from 'vitest';
import { aggregateGroupUsed, removeAggregateGroup, toggleAggregateGroups } from '../src/app/workflows/document/aggregate';
import { renameAggregateOutput } from '../src/app/workflows/document/commands';
import type { NodeDefinition, WorkflowDocument } from '../src/app/workflows/document/model';

describe.each(['iteration', 'loop'] as const)('Aggregate outputs in an enclosing %s', kind => {
  const round = (path: string[]) => kind === 'iteration'
    ? { output: { type: 'reference', path } }
    : { break_condition: { type: 'empty', value: { type: 'reference', path } } };
  function document(path: string[]): WorkflowDocument {
    const merge: NodeDefinition = { type: 'aggregate', grouped: true, groups: [
      { name: 'first', output_type: 'string', variables: [['args', 'text']] },
      { name: 'second', output_type: 'string', variables: [['args', 'text']] },
    ] };
    const container = (): NodeDefinition => {
      const reference = { type: 'reference' as const, path: [...path] };
      const body = { nodes: { start: { type: 'start' as const }, merge }, edges: [{ from: 'start', to: 'merge' }] };
      return kind === 'iteration'
        ? { type: 'iteration', iterator: { type: 'literal', value: [1] }, output: reference, body }
        : { type: 'loop', loop_count: 1, break_condition: { type: 'empty', value: reference }, body };
    };
    return { description: 'Scoped output references', block: {
      nodes: { start: { type: 'start' }, repeat: container(), sibling: container(), done: { type: 'return', outputs: [] } },
      edges: [{ from: 'start', to: 'repeat' }, { from: 'repeat', to: 'sibling' }, { from: 'sibling', to: 'done' }],
    } };
  }
  const block = [{ node: 'repeat' }];
  it('detects references held by the parent round expression', () => {
    expect(aggregateGroupUsed(document(['merge', 'second', 'output']), block, 'merge', 'second')).toBe(true);
  });
  it('renames the parent selector while leaving sibling scopes intact', () => {
    const before = document(['merge', 'second', 'output']);
    const next = renameAggregateOutput(before, block, 'merge', 'second', 'renamed');
    expect(next.block.nodes.repeat).toMatchObject(round(['merge', 'renamed', 'output']));
    expect(next.block.nodes.sibling).toEqual(before.block.nodes.sibling);
  });
  it('clears the parent selector when its output group is removed', () => {
    const before = document(['merge', 'second', 'output']);
    const next = removeAggregateGroup(before, block, 'merge', 'second');
    expect(next.block.nodes.repeat).toMatchObject(round([]));
    expect(next.block.nodes.sibling).toEqual(before.block.nodes.sibling);
  });
  it('clears inactive group selectors and remaps the first group when grouping changes', () => {
    const before = document(['merge', 'second', 'output']);
    const next = toggleAggregateGroups(before, block, 'merge', false);
    expect(next.block.nodes.repeat).toMatchObject(round([]));
    expect(next.block.nodes.sibling).toEqual(before.block.nodes.sibling);
    const ungrouped = toggleAggregateGroups(document(['merge', 'first', 'output']), block, 'merge', false);
    expect(ungrouped.block.nodes.repeat).toMatchObject(round(['merge', 'output']));
    expect(toggleAggregateGroups(ungrouped, block, 'merge', true).block.nodes.repeat).toMatchObject(round(['merge', 'first', 'output']));
  });
});
