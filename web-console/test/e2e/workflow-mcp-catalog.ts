// Read-only declarations for the Workflow editor's real HTTP MCP source.
export function payloadCatalog() {
  return { tools: [{ name: 'accept_payload', description: 'Accept a JSON object',
    inputSchema: { type: 'object', properties: { metadata: { type: 'object' } },
      required: ['metadata'], additionalProperties: false } }] };
}

export function emptyEnumCatalog() {
  return { tools: ['required', 'optional'].map(kind => ({
    name: `empty_${kind}`, description: 'Accept an explicit empty string',
    inputSchema: { type: 'object', properties: { mode: { type: 'string', enum: ['', 'explicit'] } },
      required: kind === 'required' ? ['mode'] : [], additionalProperties: false },
  })) };
}

export function pagedCatalog(cursor: string | undefined) {
  const offset = Number(cursor ?? 0);
  if (!Number.isSafeInteger(offset) || offset < 0 || offset >= 128 || offset % 16 !== 0) {
    throw new Error('Invalid fixture catalog cursor');
  }
  const properties = Object.fromEntries(Array.from({ length: 64 }, (_, field) => [
    `field_${field}`, { type: 'string', description: 'A field description. '.repeat(6) },
  ]));
  const tools = Array.from({ length: 16 }, (_, index) => ({
    name: `tool_${String(offset + index).padStart(3, '0')}`,
    description: `Catalog tool ${offset + index}`,
    inputSchema: { type: 'object', properties, additionalProperties: false },
  }));
  return { tools, ...(offset + 16 < 128 ? { nextCursor: String(offset + 16) } : {}) };
}
