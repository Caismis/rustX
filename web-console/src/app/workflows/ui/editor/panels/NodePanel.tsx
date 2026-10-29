import type { ComponentType } from 'react';
import { nodeAt, blockAt, type NodeKind } from '../../../document/model';
import { AgentPanel, ModelPanel } from './InferencePanels';
import { BranchPanel, LoopPanel, ParallelPanel } from './ControlPanels';
import { ReturnPanel, ReviewPanel, ToolPanel } from './ResultPanels';
import { Successors, type PanelProps } from './shared';

const PANELS: Record<NodeKind, ComponentType<PanelProps>> = { agent: AgentPanel, model: ModelPanel, tool: ToolPanel, branch: BranchPanel, parallel: ParallelPanel, loop: LoopPanel, return: ReturnPanel, review: ReviewPanel };
export function NodePanel(props: PanelProps) {
  const node = nodeAt(blockAt(props.document, props.block)!, props.id);
  if (!node) return null;
  const Panel = PANELS[node.type];
  return <><Panel {...props} /><Successors {...props} /></>;
}
