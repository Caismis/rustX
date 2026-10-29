import type { Connection } from '@xyflow/react';
import { CommandRejected, connectAddressed, reconnect, setEntry } from '../../document/commands';
import { blockAt, edgesOf, sameBlockPath, type WorkflowDocument } from '../../document/model';
import { edgePort, type ControlPort } from '../../document/ports';
import type { SceneEdge, SceneNode } from './scene';
import { viewId } from './selection';

/** One visual-to-native policy for connection affordances and committed gestures. */
export function applySceneConnection(current: WorkflowDocument, byId: ReadonlyMap<string, SceneNode>, connection: Connection, old?: SceneEdge): WorkflowDocument {
  if (connection.targetHandle !== 'in') throw new CommandRejected('invalid_port');
  const from = byId.get(connection.source), to = byId.get(connection.target);
  if (!from || !to || to.data.view !== 'node' || !sameBlockPath(from.data.path, to.data.path)) throw new CommandRejected('cross_scope');
  if (old && !sameBlockPath(old.data!.path, from.data.path)) throw new CommandRejected('cross_scope');
  if (old?.data?.index !== undefined) {
    const edge = edgesOf(blockAt(current, old.data.path)!)[old.data.index];
    if (!edge || viewId(old.data.path, 'node', edge.from) !== old.source || viewId(old.data.path, 'node', edge.to) !== old.target || edgePort(edge) !== old.sourceHandle) throw new CommandRejected('missing_edge');
  }
  if (from.data.view === 'start' && connection.sourceHandle === 'entry') {
    if (old?.data?.index !== undefined) throw new CommandRejected('invalid_port');
    return setEntry(current, from.data.path, to.data.local);
  }
  if (from.data.view !== 'node' || !connection.sourceHandle || old?.data?.entry) throw new CommandRejected('invalid_port');
  return old?.data?.index !== undefined ? reconnect(current, from.data.path, old.data.index, from.data.local, connection.sourceHandle as ControlPort, to.data.local)
    : connectAddressed(current, { block: from.data.path, node: from.data.local, port: connection.sourceHandle as ControlPort }, { block: to.data.path, node: to.data.local });
}
