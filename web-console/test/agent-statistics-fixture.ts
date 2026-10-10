import type { AgentStatistics } from '../../protocol/app-server/v42';
export const agentMetrics: AgentStatistics = {
  statistics: {
    turns: '3', steps: '8', completed_responses: '3', model_requests: '8', requests_with_usage: '7',
    reported_usage: { input_tokens: 10000, output_tokens: 2000, total_tokens: 12000, details: { cached_input_tokens: 8000, reasoning_tokens: 500 } },
    timing: { model_ms: 10000, tool_ms: 3000, mean_ttft_ms: 400, output_tokens_per_second: 200 },
  },
  occupancy: { input_tokens: 10000, context_window_tokens: 100000, model: 'fixture/child', breakdown: { system_tokens: 1000, tool_tokens: 2000, message_tokens: 7000 } },
  duration: { settled_ms: '15000', active: null },
};
