import { createContext, useContext } from 'react';
import type { RuntimeClientAgent, AgentStatistics } from '../../../../protocol/app-server/v57';
import type { AppServerClient } from '../../client/app-server';
export const SubagentContext = createContext<{ client: AppServerClient; sessionId?: string; onMode?: (mode: 'chat' | 'trajectory') => void; agents: RuntimeClientAgent[]; metrics: Record<string, AgentStatistics>; metricErrors: Record<string, string>; selected?: RuntimeClientAgent; opened: RuntimeClientAgent[]; openAside?: (id: string) => void; full?: boolean; headerError?: string; oneShot?: ReadonlySet<string>; openWorkflow?: (id: string, activation: string, title: string) => Promise<void>; close: (id: string) => void; open: (id?: string, source?: RuntimeClientAgent) => void } | undefined>(undefined);
export const useSubagents = () => useContext(SubagentContext);

export const AgentConversationContext = createContext<RuntimeClientAgent | undefined>(undefined);
