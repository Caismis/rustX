import { createContext, useContext } from 'react';
import type { RuntimeClientAgent, AgentStatistics } from '../../../../protocol/app-server/v41';
import type { AppServerClient } from '../../client/app-server';
export const SubagentContext = createContext<{ client: AppServerClient; sessionId?: string; agents: RuntimeClientAgent[]; metrics: Record<string, AgentStatistics>; metricErrors: Record<string, string>; selected?: RuntimeClientAgent; opened: RuntimeClientAgent[]; openAside?: (id: string) => void; open: (id?: string) => void } | undefined>(undefined);
export const useSubagents = () => useContext(SubagentContext);

export const AgentConversationContext = createContext<RuntimeClientAgent | undefined>(undefined);
