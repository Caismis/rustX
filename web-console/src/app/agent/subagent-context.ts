import { createContext, useContext } from 'react';
import type { RuntimeClientAgent } from '../../../../protocol/app-server/v37';
import type { AppServerClient } from '../../client/app-server';
export const SubagentContext = createContext<{ client: AppServerClient; sessionId?: string; agents: RuntimeClientAgent[]; selected?: RuntimeClientAgent; open: (id?: string) => void } | undefined>(undefined);
export const useSubagents = () => useContext(SubagentContext);
