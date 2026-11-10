import type { McpWrite } from '../../../../protocol/app-server/v77';

export type ResourceModule = 'subagents' | 'mcp' | 'skills';
export interface McpFocus { kind: 'mcp'; name?: string; mode?: 'json' | 'permissions' }
export type McpFieldBuffers = Partial<Record<'args' | 'env' | 'headers', string>>;
export interface McpEditorDraft {
  name: string;
  mode: 'form' | 'json';
  json?: { text: string; pinned: boolean };
  pendingImport?: { value: McpWrite; revision: string };
  invalidFields?: McpFieldBuffers;
}
