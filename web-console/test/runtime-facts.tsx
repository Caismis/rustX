import type { RuntimeClientSnapshot } from '../../protocol/app-server/v62';
import { WorkflowRunPanel } from '../src/app/agent/WorkflowRunPanel';
import { workflowKey } from '../src/bindings/workflow-children';
export function RuntimeFacts({ snapshot }: { snapshot: Pick<RuntimeClientSnapshot, 'workflows'> }) { return <>{snapshot.workflows.runs.map(run => <WorkflowRunPanel key={workflowKey(run.id)} run={run}/>)}</>; }
