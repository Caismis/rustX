import { describe, expect, it } from 'vitest';
import { buildField, fieldForm, FieldFormError, type FieldForm } from '../src/app/workflows/document/inputField';

const form = (patch: Partial<FieldForm>): FieldForm => ({ ...fieldForm('x', undefined, true), ...patch });
const problem = (value: FieldForm) => { try { buildField(value); } catch (error) { return (error as FieldFormError).problem; } return undefined; };

describe('input field form', () => {
  it('builds a string with max length, options and an optional default', () => {
    expect(buildField(form({ maxLength: '8', options: ['formal', ' casual ', 'formal'], required: false, defaultText: 'casual', description: ' Voice ' })))
      .toEqual({ type: 'string', maxLength: 8, enum: ['formal', 'casual'], description: 'Voice', default: 'casual' });
  });

  it('a required field never carries a default', () => {
    expect(buildField(form({ defaultText: 'ignored' }), { type: 'string', default: 'old' })).toEqual({ type: 'string' });
  });

  it('refuses limits and defaults the native schema would reject', () => {
    expect(problem(form({ maxLength: '0' }))).toBe('max-length-range');
    expect(problem(form({ maxLength: '65537' }))).toBe('max-length-range');
    expect(problem(form({ options: [' '] }))).toBe('options-empty');
    expect(problem(form({ required: false, maxLength: '3', defaultText: 'four' }))).toBe('default-too-long');
    expect(problem(form({ required: false, options: ['a'], defaultText: 'b' }))).toBe('default-option');
    expect(problem(form({ type: 'number', required: false, defaultText: '' }))).toBe('default-number');
    expect(problem(form({ type: 'integer', required: false, defaultText: '2.5' }))).toBe('default-integer');
    expect(problem(form({ type: 'array', required: false, defaultJson: '{}' }))).toBe('default-json');
    expect(problem(form({ type: 'object', schemaJson: '{"type":"string"}' }))).toBe('object-schema');
  });

  it('keeps keywords it does not own and round-trips through the form', () => {
    const previous = { type: 'array', items: { type: 'object', properties: {} }, const: [], default: [{}] };
    const read = fieldForm('rows', previous, false);
    expect(read).toMatchObject({ type: 'array', itemType: 'object', required: false, defaultJson: '[\n  {}\n]' });
    expect(buildField(read, previous)).toEqual(previous);
    expect(fieldForm('n', { type: 'null' }, true).type).toBe('raw');
  });
});
