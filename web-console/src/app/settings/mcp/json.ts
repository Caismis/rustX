import type { McpWrite } from '../../../../../protocol/app-server/v38';
const record = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
/** Import only native-supported fields. Never silently drop foreign options. */
export function parseMcpJson(text: string): Record<string, McpWrite> {
  let parsed: unknown;
  try { parsed = JSON.parse(text); } catch { throw new Error('Invalid JSON'); }
  if (!record(parsed)) throw new Error('Expected a JSON object');
  if (record(parsed.mcpServers) && Object.keys(parsed).some(key => key !== 'mcpServers')) throw new Error('Unsupported field');
  const servers = record(parsed.mcpServers) ? parsed.mcpServers : { '': parsed };
  const result: Record<string, McpWrite> = Object.create(null);
  for (const [name, input] of Object.entries(servers)) {
    if (!record(input)) throw new Error('Expected a server object');
    if (record(input.definition)) for (const key of Object.keys(input)) if (!['definition','retained_env','retained_headers'].includes(key)) throw new Error('Unsupported field');
    const definition = record(input.definition) ? input.definition : input;
    for (const key of ['env', 'headers']) if (definition[key] != null && (!record(definition[key]) || Object.keys(definition[key]).length)) throw new Error('Literal credentials are not supported; use environment references');
    const keys = ['type','url','command','args','cwd','env','headers','sensitive_env','sensitive_headers'];
    for (const key of Object.keys(definition)) if (!keys.includes(key)) throw new Error('Unsupported field');
    if (definition.type != null && definition.type !== 'http' && definition.type !== 'stdio') throw new Error('Unsupported transport');
    const http = definition.type === 'http' || (definition.type == null && definition.url != null);
    if (http) {
      if (typeof definition.url !== 'string' || !/^https?:\/\//.test(definition.url)) throw new Error('Expected an HTTP(S) URL');
      if (definition.command != null) throw new Error('HTTP configuration cannot include command');
    } else if (typeof definition.command !== 'string' || !definition.command.trim()) throw new Error('Expected a command');
    if (definition.args !== undefined && (!Array.isArray(definition.args) || !definition.args.every(value => typeof value === 'string'))) throw new Error('args must be an array of strings');
    if (definition.cwd != null && typeof definition.cwd !== 'string') throw new Error('cwd must be a string');
    for (const key of ['env','headers']) if (definition[key] !== undefined && (!record(definition[key]) || !Object.values(definition[key]).every(value => typeof value === 'string'))) throw new Error(`${key} must contain string values`);
    for (const key of ['sensitive_env','sensitive_headers']) if (definition[key] != null && (!record(definition[key]) || !Object.values(definition[key]).every(value => typeof value === 'string' && /^\$[A-Za-z_][A-Za-z0-9_]*$/.test(value)))) throw new Error('Expected environment variable references');
    for (const key of ['retained_env','retained_headers']) if (record(input.definition) && input[key] !== undefined && (!Array.isArray(input[key]) || !(input[key] as unknown[]).every(value => typeof value === 'string'))) throw new Error(`${key} must be an array of strings`);
    result[name] = { definition: definition as McpWrite['definition'], ...(record(input.definition) ? {retained_env:input.retained_env as string[] | undefined,retained_headers:input.retained_headers as string[] | undefined} : {}) };
  }
  if (!Object.keys(result).length) throw new Error('No MCP servers found');
  return result;
}
