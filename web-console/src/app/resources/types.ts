export type ResourceModule = 'subagents' | 'mcp' | 'skills';
export interface McpFocus { kind: 'mcp'; name?: string; mode?: 'json' | 'permissions' }
