import type { TraceToolDefinition } from '../../../protocol/app-server/v80';

export function completeTools(tools: readonly TraceToolDefinition[]) {
  return tools.every(tool => !tool.description.truncated && !tool.input_schema.truncated);
}
