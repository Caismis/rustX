import { describe, expect, it } from 'vitest';
import { emptyBlock } from '../src/app/workflows/document/commands';
import { nodeField, within } from '../src/app/workflows/document/fields';
import type { WorkflowDocument } from '../src/app/workflows/document/model';
import { orderedKeys } from '../src/app/workflows/document/presentation';
import { schemaTarget } from '../src/app/workflows/document/schema';
import { emptySchema, putProperty, readSchema, removeProperty, replaceSchema, schemaAt, schemaFromExample } from '../src/app/workflows/ui/editor/forms/structured-output/schema';

const address = nodeField([], 'model', ['output', 'schema']);
function document(): WorkflowDocument {
  const block = emptyBlock();
  block.nodes.model = { type: 'model', model: { mode: 'session' }, messages: [], output: { type: 'json_schema', schema: emptySchema() } };
  return { description: 'Schema editing.', block };
}

describe('structured output editing transactions', () => {
  it('renames declared fields and nested model-envelope references together without mutating the open document', () => {
    const original = putProperty(address, undefined, 'details', { type: 'object', properties: { accepted: { type: 'boolean' } }, required: ['accepted'], additionalProperties: false }, true)(document());
    original.block.nodes.done = { type: 'return', output: { type: 'reference', path: ['model', 'structured_output', 'details', 'accepted'] } };
    const parent = within(address, ['properties', { key: 'details' }]);
    const edited = putProperty(parent, 'accepted', 'approved', { type: 'boolean', description: 'Approval.' }, true)(original);
    expect(schemaTarget(parent)).toEqual({ kind: 'node_output', address: { block: [], node: 'model' }, path: [{ kind: 'property', name: 'structured_output' }, { kind: 'property', name: 'details' }] });
    expect(edited.block.nodes.done).toMatchObject({ output: { path: ['model', 'structured_output', 'details', 'approved'] } });
    expect(original.block.nodes.done).toMatchObject({ output: { path: ['model', 'structured_output', 'details', 'accepted'] } });
    expect(schemaAt(edited, parent)).toMatchObject({ required: ['approved'], properties: { approved: { description: 'Approval.' } } });
  });

  it('preserves imported and added field order and removes required entries with deleted fields', () => {
    let edited = replaceSchema(address, schemaFromExample({ z: 'text', a: { flag: true }, tags: ['a', 'b'] }))(document());
    edited = putProperty(address, undefined, 'middle', { type: 'string' }, true)(edited);
    expect(orderedKeys(edited, schemaTarget(address)!, Object.keys(schemaAt(edited, address).properties as object))).toEqual(['z', 'a', 'tags', 'middle']);
    edited = removeProperty(address, 'tags')(edited);
    expect(schemaAt(edited, address).required).toEqual(['z', 'a', 'middle']);
    expect(() => putProperty(address, undefined, 'z', { type: 'string' }, false)(edited)).toThrow();
  });

  it('infers example types without turning example values into constraints, and rejects mixed arrays', () => {
    expect(schemaFromExample({ text: 'hello', count: 2, ratio: 1.2, flag: true, none: null, tags: [] })).toEqual({ type: 'object', properties: {
      text: { type: 'string' }, count: { type: 'number' }, ratio: { type: 'number' }, flag: { type: 'boolean' }, none: { type: 'null' }, tags: { type: 'array', items: { type: 'string' } },
    }, required: ['text', 'count', 'ratio', 'flag', 'none', 'tags'], additionalProperties: false });
    expect(() => schemaFromExample({ items: ['text', 3] })).toThrow('mixed');
    expect(() => schemaFromExample({ items: [{ name: 'a', active: true }, { active: false, name: 'b' }] })).not.toThrow();
    expect(schemaFromExample([1, 1.5])).toEqual({ type: 'array', items: { type: 'number' } });
    expect(readSchema('{')).toEqual({ error: 'syntax' });
    expect(readSchema('{"type":"array"}')).toEqual({ error: 'root' });
  });
});
