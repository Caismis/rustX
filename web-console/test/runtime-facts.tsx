import type { RuntimeClientSnapshot } from '../../protocol/app-server/v59';
import { WorkflowRunPanel, workflowKey } from '../src/app/agent/WorkflowRunPanel';
export function RuntimeFacts({ snapshot }: { snapshot: Pick<RuntimeClientSnapshot, 'workflows'> }) { return <>{snapshot.workflows.runs.map(run => <WorkflowRunPanel key={workflowKey(run.id)} run={run}/>)}</>; }
