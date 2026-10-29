import type { ComponentType } from 'react';
import { nodeAt, blockAt, type NodeKind } from '../../../document/model';
import { AgentPanel, ModelPanel } from './InferencePanels';
import { LoopPanel } from './ControlPanels';
import { BranchPanel } from './BranchPanel';
import { ReturnPanel, ReviewPanel, ToolPanel } from './ResultPanels';
import { type PanelProps } from './shared';
import { NextStep } from '../NextStep';

const PANELS: Record<NodeKind, ComponentType<PanelProps>> = { agent: AgentPanel, model: ModelPanel, tool: ToolPanel, branch: BranchPanel, loop: LoopPanel, return: ReturnPanel, review: ReviewPanel };
export function NodePanel(props: PanelProps) {
  const node = nodeAt(blockAt(props.document, props.block)!, props.id);
  if (!node || node.type === 'start') return null;
  const Panel = PANELS[node.type];
  return <><Panel {...props} /><NextStep document={props.document} block={props.block} source={props.id} edit={props.edit} onSelect={props.onSelect} /></>;
}
