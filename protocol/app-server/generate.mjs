import {readFile, writeFile} from 'node:fs/promises';
import {compile} from 'json-schema-to-typescript';

const schema = JSON.parse(await readFile(new URL('v22.schema.json', import.meta.url), 'utf8'));
// Schemars emits draft-2020-12 $ref siblings for internally tagged newtypes.
// v21's ref resolver merges (and overwrites) their `properties`, losing the
// referenced content. Express the same conjunction using supported allOf.
// This is a compiler-input normalization, never a second wire schema.
function normalizeRefSiblings(value) {
  if (Array.isArray(value)) return value.map(normalizeRefSiblings);
  if (value === null || typeof value !== 'object') return value;
  const normalized = Object.fromEntries(Object.entries(value).map(
    ([key, child]) => [key, normalizeRefSiblings(child)],
  ));
  // Defaults are annotations, not another inferred TypeScript type (notably
  // native numeric defaults next to a lossless string-domain reference).
  delete normalized.default;
  if (normalized.$ref && (normalized.properties || normalized.required)) {
    const {$ref, description, ...siblings} = normalized;
    return {...(description ? {description} : {}), allOf: [{$ref}, siblings]};
  }
  return normalized;
}
const types = await compile(normalizeRefSiblings(schema), 'ProtocolMessage', {
  bannerComment: '// Generated from Rust App Server DTOs. Run pnpm generate in protocol/app-server. Do not edit.',
  unreachableDefinitions: true,
  // Schemars closes flattened envelopes with unevaluatedProperties. v21 does
  // not infer that keyword; omit implicit index signatures. Explicit maps
  // (additionalProperties schemas) retain their generated index signatures.
  additionalProperties: false,
  style: {singleQuote: true, printWidth: 100, tabWidth: 2},
});
await writeFile(new URL('v22.ts', import.meta.url), types);
const fixtures = await readFile(new URL('fixtures.json', import.meta.url), 'utf8');
await writeFile(new URL('fixtures.ts', import.meta.url),
  "// Generated from serialized Rust DTOs.\nimport type {ProtocolMessage} from './v22.js';\n" +
  `export const fixtures = ${fixtures.trim()} satisfies ProtocolMessage[];\n`);
