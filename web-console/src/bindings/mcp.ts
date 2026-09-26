import type { McpWrite } from '../../../protocol/app-server/v23';
/** Display native transport intent without materializing an inferred type in the draft. */
export function mcpTransport(definition: McpWrite['definition']): 'http' | 'stdio' {
  if (definition.type != null) return definition.type;
  if (definition.url != null) return 'http';
  if (definition.command != null) return 'stdio';
  return 'stdio'; // Empty/new editor only; native validation still owns admissibility.
}
