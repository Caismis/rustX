import { createContext, useState, type ReactNode } from 'react';
import type { RuntimeLayer, SourceMutation, SourceSettings } from '../../../../protocol/app-server/v16';
// Only unsaved form intent. Never persisted, merged with sources, or used as runtime state.
export interface UnitDraft { value: unknown; base: string; dirty: boolean; committed?: string; }
export const DraftContext = createContext<Map<string, UnitDraft> | undefined>(undefined);
export const SourceContext = createContext<SourceSettings | undefined>(undefined);
/** Project authored membership only. Defaults and merging remain native. */
export function authoredUnit(document: RuntimeLayer | null | undefined, mutation: SourceMutation): unknown {
  if (!document || mutation.kind !== 'config') return undefined;
  const unit = mutation.mutation;
  switch (unit.unit) {
    case 'provider': return document.providers?.[unit.id];
    case 'model': return document.models?.[unit.id];
    case 'root_model': return document.agent?.model;
    case 'native_tools': return document.agent?.tools?.builtin;
    case 'source_tools': return document.agent?.tools?.sources?.[unit.id];
    case 'skills': return document.agent?.skills;
    case 'todo': return document.agent?.plugins?.todo;
    case 'goal': return document.agent?.plugins?.goal;
    case 'agent_status': return document.agent?.plugins?.agent_status;
    case 'agents': return document.agent?.agents;
    case 'workflows': return document.agent?.workflows;
    case 'agent_identity': return document.agent_id;
    case 'description': return document.agent?.description;
    case 'instructions': return document.agent?.instructions;
    case 'project_guidance': return document.agent?.agents_md;
    case 'approval': return document.approval_mode;
    case 'context': return document.context;
    case 'model_timeout': return document.model_timeout_policy;
    case 'tool_deadline': return document.tool_deadline_policy;
    case 'capacity': return document.subagents;
    case 'native_policy': return document.native_tools?.[unit.id];
    case 'mcp_policy': return document.mcp_tool_policies?.[unit.id];
    case 'environment': return document.environment?.[unit.name];
    case 'app_server': return document.app_server;
  }
}
export function SettingsDrafts({ children }: { children: ReactNode }) {
  const [drafts] = useState(() => new Map<string, UnitDraft>());
  return <DraftContext value={drafts}>{children}</DraftContext>;
}
