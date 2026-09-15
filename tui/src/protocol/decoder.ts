/** The sole untrusted-JSON → generated DTO boundary for App Server v3. */
import { readFileSync } from "node:fs";
import { Ajv2020 } from "ajv/dist/2020.js";
import { fullFormats } from "ajv-formats/dist/formats.js";
import type { ProtocolMessage } from "../../../protocol/app-server/v3.ts";

const schema = JSON.parse(readFileSync(
  new URL("../../../protocol/app-server/v3.schema.json", import.meta.url),
  "utf8",
));
const ajv = new Ajv2020({
  // Schemars uses type unions and constraints beside references. These are
  // valid JSON Schema; Ajv's optional schema-authoring lint is inapplicable.
  strictTypes: false,
  formats: {
    "date-time": fullFormats["date-time"],
    // Rust numeric format annotations are redundant with the generated
    // integer/number types and explicit bounds, which remain enforced.
    int32: true, int64: true, uint: true, uint16: true,
    uint32: true, uint64: true, double: true,
  },
});
// Compile once. No coercion, default insertion, or removal of unknown fields.
const validate = ajv.compile<ProtocolMessage>(schema);

export function decodeProtocolMessage(record: unknown): ProtocolMessage | undefined {
  try {
    return validate(record) ? record : undefined;
  } catch {
    // Only validator execution is contained (e.g. adversarial nesting). Valid
    // semantic listeners run outside this boundary; their bugs are not wire errors.
    return undefined;
  }
}
