import type { RuntimeClientMessageDescriptor } from './v57.ts';

/** A Source-created descriptor carries positive process facts independently of its deferred body. */
export function messageDescriptor(value: unknown): RuntimeClientMessageDescriptor | undefined {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return;
  const item = value as Record<string,unknown>, locator = item.locator;
  if (!locator || typeof locator !== 'object' || Array.isArray(locator)) return;
  const position = locator as Record<string,unknown>;
  if (!(position.type === 'by_message_id' && typeof position.message_id === 'string'
    || position.type === 'by_ledger_position' && Number.isSafeInteger(position.position) && Number(position.position) >= 0)) return;
  if (!['user','assistant','tool'].includes(String(item.role)) || typeof item.kind !== 'string'
    || typeof item.block_count !== 'string' || typeof item.tool_call_count !== 'string'
    || !Array.isArray(item.tool_calls) || item.tool_calls.length > 32
    || !Array.isArray(item.content) || !Array.isArray(item.collections)) return;
  return item as unknown as RuntimeClientMessageDescriptor;
}
