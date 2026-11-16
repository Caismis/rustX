import { createContext, useContext } from 'react';
import type {
  CheckpointReadCut,
  TraceRecord,
  WorkflowNodeInstance,
  WorkflowRunId,
  WorkflowTraceNode,
} from '../../../../../protocol/app-server/v78';
import type { TraceFieldReader } from '../../../client/trace';
import type { WorkflowRunSnapshotTransport } from './contract';

export interface WorkflowRunCanvasTarget {
  readonly runId?: WorkflowRunId;
  readonly workflowId?: string;
  readonly conversationId: string;
  readonly selectedVisit?: WorkflowNodeInstance;
  readonly selectedTraceNode?: WorkflowTraceNode;
  readonly traceRoot?: TraceRecord;
  readonly traceNodes?: readonly WorkflowTraceNode[];
  readonly traceIncomplete?: boolean;
  readonly traceReader?: TraceFieldReader;
  readonly traceCut?: CheckpointReadCut;
  readonly sourceMode: 'live' | 'history';
  readonly transport?: WorkflowRunSnapshotTransport;
}

export interface WorkflowRunCanvasActions {
  openRunCanvas(target: WorkflowRunCanvasTarget, trigger?: HTMLElement | null): void;
}

export const WorkflowRunCanvasContext = createContext<WorkflowRunCanvasActions | undefined>(undefined);

export function useWorkflowRunCanvas() {
  return useContext(WorkflowRunCanvasContext);
}
