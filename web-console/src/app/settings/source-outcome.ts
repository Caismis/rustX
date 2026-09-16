import type { McpDraft, SourceMutation, SourceSettings } from '../../../../protocol/app-server/v5';
// Equality of an exact typed source target after uncertainty, never resolution,
// merge or provenance inference. A match proves the requested state is present;
// it does not attribute that state to a particular writer.
export function observesSourceMutation(source: SourceSettings, mutation: SourceMutation): boolean {
  if (mutation.kind === 'mcp_policy') {
    const observed = source.integrations.mcp_tool_policies[mutation.id];
    const requested = mutation.authored;
    if (!requested) return observed == null;
    return observed != null && (['approval', 'execution', 'concurrency'] as const).every(key => observed[key] === requested[key]);
  }
  if (mutation.kind !== 'mcp') return false;
  const observed = source.integrations.mcp.find(entry => entry.id === mutation.id)?.[mutation.scope];
  if (!mutation.authored) return observed == null;
  if (!observed) return false;
  const requested: McpDraft = mutation.authored;
  return (['enabled', 'transport', 'command', 'cwd', 'url'] as const).every(key => observed[key] === requested[key])
    && (['args', 'retained_env', 'retained_headers'] as const).every(key => observed[key].length === requested[key].length && observed[key].every((value, i) => value === requested[key][i]))
    && (['sensitive_env', 'sensitive_headers'] as const).every(key => Object.keys(observed[key]).length === Object.keys(requested[key]).length && Object.entries(observed[key]).every(([name, value]) => requested[key][name] === value));
}
