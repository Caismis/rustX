import {readFile, writeFile} from 'node:fs/promises';
import {compile} from 'json-schema-to-typescript';

const schema = JSON.parse(await readFile(new URL('v1.schema.json', import.meta.url), 'utf8'));
const types = await compile(schema, 'ProtocolMessage', {
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
