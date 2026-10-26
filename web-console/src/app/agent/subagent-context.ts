import { createContext, useContext } from 'react';
import type { RuntimeClientAgent, AgentStatistics } from '../../../../protocol/app-server/v60';
import type { AppServerClient } from '../../client/app-server';
import type { WorkflowChild } from '../../bindings/workflow-children';
import type { useWorkflowContent } from './useWorkflowContent';
export const SubagentContext = createContext<{ client: AppServerClient; sessionId?: string; onMode?: (mode: 'chat' | 'trajectory') => void; agents: RuntimeClientAgent[]; workflows?: ReturnType<typeof useWorkflowContent>; workflowChildren?: readonly WorkflowChild[]; metrics: Record<string, AgentStatistics>; metricErrors: Record<string, string>; selected?: RuntimeClientAgent; opened: RuntimeClientAgent[]; openAside?: (id: string) => void; full?: boolean; headerError?: string; oneShot?: ReadonlySet<string>; openWorkflow?: (id: string, activation: string, title: string) => Promise<void>; close: (id: string) => void; open: (id?: string, source?: RuntimeClientAgent) => void } | undefined>(undefined);
export const useSubagents = () => useContext(SubagentContext);

export const AgentConversationContext = createContext<RuntimeClientAgent | undefined>(undefined);
