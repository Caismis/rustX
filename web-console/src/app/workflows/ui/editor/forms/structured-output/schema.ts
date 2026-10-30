import { readJson } from '../../../../../../../shared/json-numbers';
import { CommandRejected } from '../../../../document/commands';
import { fieldValue, setField, within, type FieldAddress } from '../../../../document/fields';
import { setOrder } from '../../../../document/geometry';
import type { WorkflowDocument } from '../../../../document/model';
import { orderedKeys } from '../../../../document/presentation';
import { renameSchemaField, schemaObject, schemaTarget, schemaTemplate, type Schema } from '../../../../document/schema';

export type SchemaEdit = (document: WorkflowDocument) => WorkflowDocument;
export const emptySchema = () => schemaTemplate('object');
export function schemaAt(document: WorkflowDocument, address: FieldAddress): Schema {
  return schemaObject(fieldValue(document, address)?.value) ?? {};
}

/** Reuse the document's schema commands so field renames move references and
 * presentation order in the same transaction as the declared output. */
export function putProperty(parent: FieldAddress, oldName: string | undefined, name: string, schema: Schema, required: boolean): SchemaEdit {
  return document => {
    let next = oldName !== undefined && oldName !== name ? renameSchemaField(document, parent, oldName, name) : document;
    const object = schemaAt(next, parent), properties = schemaObject(object.properties) ?? {};
    if (oldName === undefined && Object.hasOwn(properties, name)) throw new CommandRejected('duplicate_identifier');
    const target = schemaTarget(parent), keys = target ? orderedKeys(next, target, Object.keys(properties)) : Object.keys(properties);
    const requiredFields = (Array.isArray(object.required) ? object.required : []).filter(field => field !== name);
    next = setField(next, parent, { ...object, properties: { ...properties, [name]: schema }, required: required ? [...requiredFields, name] : requiredFields });
    return target ? setOrder(next, target, oldName === undefined ? [...keys, name] : keys) : next;
  };
}

export function removeProperty(parent: FieldAddress, name: string): SchemaEdit {
  return document => {
    const object = schemaAt(document, parent), properties = schemaObject(object.properties) ?? {};
    const next = setField(document, parent, { ...object,
      properties: Object.fromEntries(Object.entries(properties).filter(([key]) => key !== name)),
      required: (Array.isArray(object.required) ? object.required : []).filter(key => key !== name),
    });
    const target = schemaTarget(parent);
    return target ? setOrder(next, target, orderedKeys(document, target, Object.keys(properties)).filter(key => key !== name)) : next;
  };
}

/** Imported map order becomes explicit editor metadata, including nested
 * objects. The native schema bytes themselves are kept unchanged. */
export function replaceSchema(address: FieldAddress, schema: Schema): SchemaEdit {
  return document => {
    let next = setField(document, address, schema);
    const order = (at: FieldAddress, value: Schema, depth: number) => {
      if (depth > 16) return;
      const properties = schemaObject(value.properties), target = schemaTarget(at);
      if (properties) {
        if (target) next = setOrder(next, target, Object.keys(properties));
        for (const [key, child] of Object.entries(properties)) if (schemaObject(child)) order(within(at, ['properties', { key }]), child as Schema, depth + 1);
      }
      if (schemaObject(value.items)) order(within(at, ['items']), value.items as Schema, depth + 1);
    };
    order(address, schema, 0);
    return next;
  };
}

export type Reading = { schema: Schema; error?: never } | { error: 'syntax' | 'number' | 'root'; schema?: never };
export function readSchema(text: string): Reading {
  const result = readJson(text);
  if (!result.ok) return { error: result.reason };
  const schema = schemaObject(result.value);
  return schema?.type === 'object' ? { schema } : { error: 'root' };
}

/** An example supplies shape, never const values. Every property is required,
 * matching Dify's JSON import; empty arrays start with string items. */
export function schemaFromExample(value: unknown, depth = 0): Schema {
  if (depth > 16) throw new Error('depth');
  if (value === null) return { type: 'null' };
  if (Array.isArray(value)) {
    const items = value.length ? schemaFromExample(value[0], depth + 1) : { type: 'string' };
    if (value.slice(1).some(item => !sameExampleShape(schemaFromExample(item, depth + 1), items))) throw new Error('mixed');
    return { type: 'array', items };
  }
  const object = schemaObject(value);
  if (object) return { type: 'object', properties: Object.fromEntries(Object.entries(object).map(([key, value]) => [key, schemaFromExample(value, depth + 1)])), required: Object.keys(object), additionalProperties: false };
  return { type: typeof value };
}

function sameExampleShape(left: Schema, right: Schema): boolean {
  if (left.type !== right.type) return false;
  if (left.type === 'array') return sameExampleShape(left.items as Schema, right.items as Schema);
  if (left.type !== 'object') return true;
  const a = left.properties as Record<string, Schema>, b = right.properties as Record<string, Schema>;
  return Object.keys(a).length === Object.keys(b).length && Object.entries(a).every(([key, value]) => Object.hasOwn(b, key) && sameExampleShape(value, b[key]));
}
