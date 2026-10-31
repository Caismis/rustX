import type { ComponentType } from 'react';
import { nodeAt, blockAt, type NodeKind } from '../../../document/model';
import { AgentPanel, ModelPanel } from './InferencePanels';
import { AssignerPanel, IterationPanel, LoopEndPanel, LoopPanel } from './ControlPanels';
import { BranchPanel } from './BranchPanel';
import { ReturnPanel } from './ResultPanels';
import { ReviewPanel } from './ReviewPanel';
import { ToolPanel } from './ToolPanel';
import { type PanelProps } from './shared';
import { NextStep } from '../NextStep';
import css from '../Editor.module.css';

const PANELS: Record<NodeKind, ComponentType<PanelProps>> = { agent: AgentPanel, model: ModelPanel, tool: ToolPanel, branch: BranchPanel, loop: LoopPanel,
  iteration: IterationPanel, loop_end: LoopEndPanel, assigner: AssignerPanel, return: ReturnPanel, review: ReviewPanel };
export function NodePanel(props: PanelProps) {
  const node = nodeAt(blockAt(props.document, props.block)!, props.id);
  if (!node || node.type === 'start') return null;
  const Panel = PANELS[node.type];
  // As Dify's End, Return is the last node: nothing follows it.
  return <div className={css.nodeSettings}><Panel {...props} />{node.type !== 'return' && <NextStep document={props.document} block={props.block} source={props.id} tools={props.vocabulary?.builtin_tools ?? []} edit={props.edit} onSelect={props.onSelect} />}</div>;
}
