import type {
  WorkflowDefinitionPath,
  WorkflowNodeInstance,
  WorkflowRunId,
} from '../../../../../protocol/app-server/v78';
import { workflowKey } from '../../../bindings/workflow-children';
import type { BlockPath, BlockStep } from '../document/model';
import type { SceneNode } from '../ui/editor/scene';
import { viewId } from '../ui/editor/selection';

/** Convert native alternating `[ownerNode, "body", ...]` definition segments
 * into the editor scene's lexical BlockPath. */
export function blockPathFromDefinition(definition: Pick<WorkflowDefinitionPath, 'blocks'>): BlockPath {
  const steps: BlockStep[] = [];
  const raw = definition.blocks;
  for (let index = 0; index < raw.length; index += 2) {
    const node = raw[index];
    if (node) steps.push({ node });
  }
  return steps;
}

export function qualifiedNodeViewId(blocks: readonly string[], node: string): string {
  return viewId(blockPathFromDefinition({ blocks: [...blocks] }), 'node', node);
}

export function sameWorkflowRunId(a: WorkflowRunId, b: WorkflowRunId): boolean {
  return workflowKey(a) === workflowKey(b);
}

export function visitIdentityKey(instance: Pick<WorkflowNodeInstance, 'block' | 'node' | 'visit'>): string {
  return JSON.stringify([
    workflowKey(instance.block.run),
    instance.block.definition.workflow_id,
    instance.block.definition.blocks,
    instance.block.invocations,
    instance.node,
    instance.visit,
  ]);
}

/** Resolve the exact scene node id for a runtime/trace node instance.
 * Synthetic editor-only nodes (`note`) never match executable visits;
 * executable Start nodes (`workflow_start`, `loop_start`, etc.) match the
 * projected Start SceneNode at the exact Block path whose `data.local` equals `instance.node`. */
export function resolveSceneNodeId(
  sceneNodes: readonly SceneNode[],
  instance: Pick<WorkflowNodeInstance, 'block' | 'node'>,
): string | undefined {
  const path = blockPathFromDefinition(instance.block.definition);
  const candidateNodeId = viewId(path, 'node', instance.node);
  const direct = sceneNodes.find(node => node.id === candidateNodeId && node.data.view === 'node');
  if (direct) return direct.id;
  const startId = viewId(path, 'start');
  const startNode = sceneNodes.find(
    node => node.id === startId && node.data.view === 'start' && node.data.local === instance.node,
  );
  return startNode?.id;
}
