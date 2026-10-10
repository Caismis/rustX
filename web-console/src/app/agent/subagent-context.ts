import { createContext, useContext } from 'react';
import type { RuntimeClientAgent, AgentStatistics } from '../../../../protocol/app-server/v39';
import type { AppServerClient } from '../../client/app-server';
export const SubagentContext = createContext<{ client: AppServerClient; sessionId?: string; agents: RuntimeClientAgent[]; metrics: Record<string, AgentStatistics>; metricErrors: Record<string, string>; selected?: RuntimeClientAgent; opened: RuntimeClientAgent[]; open: (id?: string) => void } | undefined>(undefined);
export const useSubagents = () => useContext(SubagentContext);
