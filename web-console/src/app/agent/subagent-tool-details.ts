import type { RuntimeClientAgent, SubagentState, ForegroundToolExecution } from '../../../../protocol/app-server/v41';
import type { TranslationKey } from '../../locale/translation';

export interface AgentToolItem {
  name?: string; agentId?: string; text?: string;
  status?: TranslationKey; tone?: 'ongoing' | 'done' | 'warning' | 'error' | 'idle';
}
export interface AgentToolDetails {
  target?: string; task?: string; items: AgentToolItem[];
  count?: number; truncated?: { returned: number; matched: number };
}
const object = (value: unknown): Record<string, unknown> | undefined => value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
const string = (value: unknown) => typeof value === 'string' ? value : undefined;
const states: Record<string, Pick<AgentToolItem, 'status' | 'tone'>> = {
  active: { status: 'common:subagents.active', tone: 'ongoing' },
  inactive: { status: 'common:subagents.inactive', tone: 'warning' },
  admitting: { status: 'common:activity.admitting', tone: 'ongoing' },
  stopping: { status: 'common:activity.stopping', tone: 'warning' },
  unavailable: { status: 'common:activity.unavailable', tone: 'error' },
  succeeded: { status: 'common:state.succeeded', tone: 'done' },
  failed: { status: 'common:state.failed', tone: 'error' },
  cancelled: { status: 'common:state.cancelled', tone: 'warning' },
  interrupted: { status: 'common:state.interrupted', tone: 'error' },
  running: { status: 'common:state.running', tone: 'ongoing' },
  cancelling: { status: 'common:state.cancelling', tone: 'warning' },
  publishing_terminal: { status: 'common:state.publishing_terminal', tone: 'ongoing' },
} satisfies Record<RuntimeClientAgent['state'] | SubagentState, Pick<AgentToolItem, 'status' | 'tone'>>;
/** Adapt recorded native ToolResult JSON, never text diagnostics or live Agent state.
 * A successful RPC is a receipt; it does not establish that the child succeeded. */
export function subagentToolDetails(tool: ForegroundToolExecution): AgentToolDetails {
  let input: Record<string, unknown> | undefined;
  try { input = object(JSON.parse(tool.state.arguments)); } catch { /* Partial streaming arguments. */ }
  const task = string(input?.task) ?? string(input?.message);
  const target = string(input?.agent_id);
  const details: AgentToolDetails = { target, task, items: [] };
  if (tool.state.type !== 'settled' || tool.state.result.status.type !== 'success') return details;
  const result = tool.state.result.content?.find(block => block.type === 'json');
  const value = result?.type === 'json' ? object(result.value) : undefined;
  if (!value) return details;
  details.target ??= string(value.agent_id);
  if (tool.tool_id === 'tool-list_agents' && Array.isArray(value.agents)) {
    details.count = value.agents.length;
    details.items = value.agents.flatMap(entry => {
      const agent = object(entry), agentId = string(agent?.agent_id);
      return agent && agentId ? [{ agentId, name: string(agent.title), ...states[string(agent.state) ?? ''] }] : [];
    });
    if (value.truncated === true && typeof value.returned === 'number' && typeof value.matched === 'number') details.truncated = { returned: value.returned, matched: value.matched };
  } else if (tool.tool_id === 'tool-subagent' && details.target) {
    details.items = [{ agentId: details.target, name: string(input?.title), text: task, status: 'common:subagents.started', tone: 'ongoing' }];
  } else if (tool.tool_id === 'tool-send_message' && details.target) {
    details.items = [{ agentId: details.target, text: task, status: 'common:subagents.accepted', tone: 'done' }];
  } else if ((tool.tool_id === 'tool-wait_agent' || tool.tool_id === 'tool-interrupt_agent') && details.target) {
    details.items = [{ agentId: details.target, ...(value.outcome === null ? states.inactive : states[string(value.outcome) ?? '']) }];
  }
  return details;
}
