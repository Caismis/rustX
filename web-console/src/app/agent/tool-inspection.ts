import { createContext, useContext, useLayoutEffect, useRef } from 'react';
import type { ForegroundToolExecution, TraceToolLocator } from '../../../../protocol/app-server/v57';

/** The owning Conversation supplies the native navigation authority. */
export const ToolInspectionContext = createContext<((locator: TraceToolLocator) => Promise<void>) | undefined>(undefined);
export function useToolInspection(tool: ForegroundToolExecution) {
  const locate = useContext(ToolInspectionContext);
  const callId=tool.call_id,toolId=tool.tool_id;
  return locate ? () => locate({ occurrence: tool.occurrence, call_id:callId, tool_id:toolId }) : undefined;
}

/** A still-attached background Session must not switch the visible Conversation's tab. */
export function useToolTraceNavigation(locate: (locator: TraceToolLocator) => Promise<boolean>, onMode: ((mode: 'chat' | 'trajectory') => void) | undefined, enabled: boolean, seat: string) {
  const owner = useRef<object | undefined>(undefined);
  useLayoutEffect(() => {
    const token = {};
    owner.current = enabled ? token : undefined;
    return () => { if (owner.current === token) owner.current = undefined; };
  }, [seat, enabled]);
  return enabled && onMode ? async (locator: TraceToolLocator) => {
    const token = owner.current;
    if (token && await locate(locator) && owner.current === token) onMode('trajectory');
  } : undefined;
}
