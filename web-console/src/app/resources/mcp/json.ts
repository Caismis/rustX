import type { McpWrite } from '../../../../../protocol/app-server/v78';
const record = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
const PERMISSION_VALUES = new Set(['default', 'allow', 'ask', 'deny']);

/** Complete three-mode permission object: all modes required, native enum only. */
function parseResourcePermissions(value: unknown, label: string): void {
  if (!record(value)) throw new Error(`${label} must be an object`);
  for (const mode of ['plan', 'automatic', 'full_access'] as const) {
    if (!Object.hasOwn(value, mode)) throw new Error(`${label} must include plan, automatic, and full_access`);
    if (!PERMISSION_VALUES.has(value[mode] as string)) {
      throw new Error('permission values must be default, allow, ask, or deny');
    }
  }
  for (const key of Object.keys(value)) if (!['plan', 'automatic', 'full_access'].includes(key)) throw new Error('Unsupported field');
}

/** Import only native-supported fields. Never silently drop foreign options. */
export function parseMcpJson(text: string): Record<string, McpWrite> {
  let parsed: unknown;
  try { parsed = JSON.parse(text); } catch { throw new Error('Invalid JSON'); }
  if (!record(parsed)) throw new Error('Expected a JSON object');
  if (record(parsed.mcpServers) && Object.keys(parsed).some(key => key !== 'mcpServers')) throw new Error('Unsupported field');
  const direct = ['type','command','url','definition'].some(key => Object.hasOwn(parsed, key) && !record(parsed[key]));
  const servers = record(parsed.mcpServers) ? parsed.mcpServers : direct || record(parsed.definition) ? { '': parsed } : parsed;
  const result: Record<string, McpWrite> = Object.create(null);
  for (const [name, input] of Object.entries(servers)) {
    if (!record(input)) throw new Error('Expected a server object');
    if (record(input.definition)) for (const key of Object.keys(input)) if (!['definition','retained_env','retained_headers'].includes(key)) throw new Error('Unsupported field');
    const definition = record(input.definition) ? input.definition : input;
    for (const key of ['env', 'headers']) if (definition[key] != null && (!record(definition[key]) || Object.keys(definition[key]).length)) throw new Error('Literal credentials are not supported; use environment references');
    const keys = ['type','url','command','args','cwd','env','headers','sensitive_env','sensitive_headers','permissions','tools'];
    for (const key of Object.keys(definition)) if (!keys.includes(key)) throw new Error('Unsupported field');
    // Omitted or null permissions inherit native defaults; an authored object
    // must supply the complete three-mode ResourcePermissionSet, including default.
    if (definition.permissions != null) parseResourcePermissions(definition.permissions, 'permissions');
    if (definition.tools != null) {
      if (!record(definition.tools)) throw new Error('tools must be an object');
      for (const tool of Object.values(definition.tools)) {
        if (!record(tool)) throw new Error('each tool must be an object');
        for (const key of Object.keys(tool)) if (key !== 'permissions') throw new Error('Unsupported field');
        // Exact-tool permissions are optional/nullable; `{}` and `{permissions:null}` are legal.
        if (tool.permissions != null) parseResourcePermissions(tool.permissions, 'tool permissions');
      }
    }
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

/** Named configuration for the editor; private native retention metadata stays inside the transaction. */
export function formatMcpJson(name: string, value: McpWrite): string {
  return JSON.stringify({[name || 'my-mcp-server']: value.definition}, null, 2);
}
