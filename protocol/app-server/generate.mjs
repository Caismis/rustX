import {readFile, writeFile} from 'node:fs/promises';
import {compile} from 'json-schema-to-typescript';

/**
 * Rewrites JSON Schema's sibling-`$ref` composition into the equivalent
 * `allOf` form.
 *
 * Schemars emits an internally tagged newtype variant as one object carrying
 * both the discriminant `properties` and a `$ref` to the variant's payload
 * struct. That is correct draft 2020-12 — every keyword applies — and the Rust
 * round-trip and schema validation already honour it. `json-schema-to-typescript`
 * v16 does not: given a sibling `$ref` it emits only the local `properties` and
 * silently drops the payload, so `MessageBlock` would reach clients as a bare
 * `{ role }` with no message content at all.
 *
 * The rewrite is a lossless projection of the same constraints into the shape
 * the emitter does understand, applied to the schema that feeds code generation
 * only. The committed `v1.schema.json` stays exactly as Rust derived it, and it
 * remains the single authority for validation.
 */
function composeSiblingRefs(node) {
  if (Array.isArray(node)) return node.map(composeSiblingRefs);
  if (node === null || typeof node !== 'object') return node;
  const entries = Object.fromEntries(
    Object.entries(node).map(([key, value]) => [key, composeSiblingRefs(value)]),
  );
  const {$ref, description, ...rest} = entries;
  // `default` beside a `$ref` constrains nothing and needs no composition.
  const composable = Object.keys(rest).filter((key) => key !== 'default');
  if ($ref === undefined || composable.length === 0) return entries;
  const {default: fallback, ...constraints} = rest;
  return {
    ...(description === undefined ? {} : {description}),
    ...(fallback === undefined ? {} : {default: fallback}),
    allOf: [{$ref}, constraints],
  };
}

const schema = JSON.parse(await readFile(new URL('v1.schema.json', import.meta.url), 'utf8'));
const types = await compile(composeSiblingRefs(schema), 'ProtocolMessage', {
  bannerComment: '// Generated from Rust App Server DTOs. Run pnpm generate in protocol/app-server. Do not edit.',
  unreachableDefinitions: true,
  // Schemars closes flattened envelopes with unevaluatedProperties. v16 does
  // not infer that keyword; omit implicit index signatures. Explicit maps
  // (additionalProperties schemas) retain their generated index signatures.
  additionalProperties: false,
  style: {singleQuote: true, printWidth: 100, tabWidth: 2},
});
await writeFile(new URL('v1.ts', import.meta.url), types);
const fixtures = await readFile(new URL('fixtures.json', import.meta.url), 'utf8');
await writeFile(new URL('fixtures.ts', import.meta.url),
  "// Generated from serialized Rust DTOs.\nimport type {ProtocolMessage} from './v1.js';\n" +
  `export const fixtures = ${fixtures.trim()} satisfies ProtocolMessage[];\n`);
