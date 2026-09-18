import type { ForegroundToolExecution } from '../../../../protocol/app-server/v6';
import { toolCard } from '../../bindings/tools';
import { ToolCard } from '../../presentation/agent/ToolCard';
import { ToolArtifacts } from '../components/Artifact';
export function Tool({ tool }: { tool: ForegroundToolExecution }) {
  return <ToolCard tool={{ ...toolCard(tool), artifacts: tool.state.type === 'settled' ? <ToolArtifacts result={tool.state.result}/> : undefined }}/>;
}
