import type { McpWrite } from '../../../protocol/app-server/v80';
/** Display native transport intent without materializing an inferred type in the draft. */
export function mcpTransport(definition: McpWrite['definition']): 'http' | 'stdio' {
  if (definition.type != null) return definition.type;
  if (definition.url != null) return 'http';
  if (definition.command != null) return 'stdio';
  return 'stdio'; // Empty/new editor only; native validation still owns admissibility.
}

function canonical(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>)
        .filter(([, entry]) => entry !== undefined)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([key, entry]) => [key, canonical(entry)]),
    );
  }
  return value;
}

/** Stable identity for an MCP server's connection settings, excluding approval
 * rules so editing server or per-tool permissions does not trigger rediscovery. */
export function mcpConnectionIdentity(entry?: McpWrite, revision?: string): string | undefined {
  if (!entry) return undefined;
  const { permissions: _permissions, tools: _tools, ...connection } = entry.definition;
  const env = [...(entry.retained_env ?? [])].sort();
  const headers = [...(entry.retained_headers ?? [])].sort();
  // The redacted view omits credential values. A revision change may have
  // changed the active credentials even if their names stayed the same.
  return JSON.stringify([
    canonical(connection),
    env,
    headers,
    env.length || headers.length ? revision : undefined,
  ]);
}
