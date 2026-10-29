import { readJson } from '../../../../shared/json-numbers';
import { schemaObject, schemaTemplate, type Schema } from './schema';

/** The native schema types an input field can take. `null` carries no input,
 * so it is edited only through Advanced JSON. */
export const FIELD_TYPES = ['string', 'number', 'integer', 'boolean', 'array', 'object'] as const;
export type FieldType = typeof FIELD_TYPES[number];
/** Item types of an array field; '' leaves items unconstrained. */
export const ITEM_TYPES = ['', 'string', 'number', 'integer', 'boolean', 'object'] as const;
/** As the native `maxLength` bound (MAX_VALUE_BYTES). */
export const MAX_LENGTH_LIMIT = 65536;

/** One input field as its dialog edits it. `raw` is a property whose type the
 * dialog does not model; its whole schema is edited as JSON. */
export interface FieldForm {
  type: FieldType | 'raw';
  name: string;
  description: string;
  required: boolean;
  maxLength: string;
  options: string[];
  defaultText: string;
  defaultBool: boolean;
  defaultJson: string;
  itemType: string;
  schemaJson: string;
}

export type FieldProblem = 'json-invalid' | 'object-schema' | 'max-length-range' | 'options-empty' | 'default-too-long'
  | 'default-option' | 'default-number' | 'default-integer' | 'default-json';
export class FieldFormError extends Error {
  constructor(readonly problem: FieldProblem) { super(problem); }
}

const json = (value: unknown) => JSON.stringify(value, null, 2);
const omit = (schema: Schema, keys: readonly string[]) => Object.fromEntries(Object.entries(schema).filter(([key]) => !keys.includes(key)));
/** Keywords the dialog owns; any other keyword of the same type survives an edit. */
const OWNED = ['description', 'default', 'enum', 'maxLength', 'items'];

export function fieldForm(name: string, schema: Schema | undefined, required: boolean): FieldForm {
  const type = !schema ? 'string' : FIELD_TYPES.find(type => type === schema.type) ?? 'raw';
  const fallback = schema?.default;
  const items = schemaObject(schema?.items);
  return {
    type, name, required: schema ? required : true,
    description: typeof schema?.description === 'string' ? schema.description : '',
    maxLength: typeof schema?.maxLength === 'number' ? String(schema.maxLength) : '',
    options: Array.isArray(schema?.enum) ? schema.enum.filter(option => typeof option === 'string') : [],
    defaultText: typeof fallback === 'string' || typeof fallback === 'number' ? String(fallback) : '',
    defaultBool: fallback === true,
    defaultJson: fallback !== undefined && typeof fallback === 'object' ? json(fallback) : type === 'array' ? '[]' : '{}',
    itemType: typeof items?.type === 'string' ? items.type : schema?.type === 'array' ? '' : 'string',
    schemaJson: json(type === 'raw' ? schema : type === 'object' ? omit(schema!, ['description', 'default']) : schemaTemplate('object')),
  };
}

function parseObject(text: string): Schema {
  const reading = readJson(text), value = reading.ok ? schemaObject(reading.value) : undefined;
  if (!value) throw new FieldFormError('json-invalid');
  return value;
}

/** The property schema `form` describes, keeping keywords the dialog does
 * not own when the type is unchanged. A required field has no default; an
 * optional one always has one, filled in natively when the caller omits it. */
export function buildField(form: FieldForm, previous?: Schema): Schema {
  if (form.type === 'raw') return parseObject(form.schemaJson);
  const type = form.type;
  let next: Schema;
  if (type === 'object') {
    next = omit(parseObject(form.schemaJson), ['description', 'default']);
    if (next.type !== 'object') throw new FieldFormError('object-schema');
  } else next = { ...omit(previous?.type === type ? previous : schemaTemplate(type), OWNED), type };
  if (type === 'string') {
    if (form.maxLength.trim()) {
      const max = Number(form.maxLength);
      if (!Number.isInteger(max) || max < 1 || max > MAX_LENGTH_LIMIT) throw new FieldFormError('max-length-range');
      next.maxLength = max;
    }
    const options = [...new Set(form.options.map(option => option.trim()).filter(Boolean))];
    if (form.options.length && !options.length) throw new FieldFormError('options-empty');
    if (options.length) next.enum = options;
  }
  if (type === 'array' && form.itemType) {
    const items = schemaObject(previous?.items);
    next.items = previous?.type === 'array' && items?.type === form.itemType ? items : schemaTemplate(form.itemType);
  }
  if (form.description.trim()) next.description = form.description.trim();
  if (!form.required) next.default = parseDefault(form, next);
  return next;
}

function parseDefault(form: FieldForm, schema: Schema): unknown {
  switch (form.type) {
    case 'string': {
      const options = schema.enum as string[] | undefined;
      if (options && !options.includes(form.defaultText)) throw new FieldFormError('default-option');
      if (typeof schema.maxLength === 'number' && [...form.defaultText].length > schema.maxLength) throw new FieldFormError('default-too-long');
      return form.defaultText;
    }
    case 'number': case 'integer': {
      const reading = form.defaultText.trim() ? readJson(form.defaultText.trim()) : undefined;
      if (!reading?.ok || typeof reading.value !== 'number') throw new FieldFormError('default-number');
      if (form.type === 'integer' && !Number.isInteger(reading.value)) throw new FieldFormError('default-integer');
      return reading.value;
    }
    case 'boolean': return form.defaultBool;
    default: {
      const reading = readJson(form.defaultJson);
      const shaped = reading.ok && (form.type === 'array' ? Array.isArray(reading.value) : schemaObject(reading.value));
      if (!reading.ok || !shaped) throw new FieldFormError('default-json');
      return reading.value;
    }
  }
}
