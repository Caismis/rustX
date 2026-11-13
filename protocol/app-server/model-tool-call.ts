/** The model's tool-call wire envelope. This is a display projection only;
 * native preflight remains the authority for the invocation and its metadata.
 * Workflow node invocations and approval facts already carry business arguments
 * and must never pass through this model-call boundary. */
export interface ModelToolCall {
  readonly arguments: Record<string, unknown>;
  readonly execution_mode?: unknown;
  readonly elevation_reason?: unknown;
}
const object = (value: unknown): value is Record<string, unknown> =>
  value !== null && typeof value === 'object' && !Array.isArray(value);

/** Extract one envelope, without interpreting business fields as metadata. */
export function modelToolCall(value: unknown): ModelToolCall | undefined {
  return object(value) && object(value.arguments) ? value as unknown as ModelToolCall : undefined;
}

/** Incomplete streaming JSON has no business-argument projection yet. */
export function parseModelToolCall(raw: string): ModelToolCall | undefined {
  try { return modelToolCall(JSON.parse(raw)); } catch { return undefined; }
}
