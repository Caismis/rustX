import { useEffect, useRef, useState } from 'react';
import type { InspectableDisplayItem, TrajectoryFacet } from './layout';
import { readTraceSource } from '../../client/trace-content';
import { parseJson } from '../../presentation/primitives/json';
import type { TraceFieldReader } from '../../client/trace';

export function useWorkflowNodeContent(item: InspectableDisplayItem, facet: TrajectoryFacet) {
  const source = item.type === 'WorkflowNode' ? item.workflowSource : undefined;
  const node = item.type === 'WorkflowNode' ? item.workflowNode : undefined;
  const fields = node
    ? facet === 'input'
      ? [['input', node.input] as const]
      : facet === 'output'
        ? [['output', node.output] as const]
        : facet === 'overview'
          ? [['input', node.input] as const, ['output', node.output] as const]
          : []
    : [];
  const owner = JSON.stringify([source?.cut.conversation_id, source?.cut.activation_id, node?.id, fields]);
  const identity = JSON.stringify([owner, source?.cut]);
  const latest = useRef({ identity, reader: source?.reader });
  latest.current = { identity, reader: source?.reader };
  const [state, setState] = useState<{
    identity: string;
    owner: string;
    reader?: TraceFieldReader;
    values?: Record<string, unknown>;
    error?: string;
  }>();
  useEffect(() => {
    if (!source || !node) return;
    let mounted = true;
    const current = () => mounted && latest.current.identity === identity && latest.current.reader === source.reader;
    void Promise.all(
      fields
        .filter(([, field]) => field != null)
        .map(async ([key, field]) => {
          const value = await readTraceSource(source.reader, source.cut, field!, current);
          if (typeof value !== 'string') throw Error('Original Workflow node body is unavailable.');
          return [key, parseJson(value)] as const;
        }),
    )
      .then((values) => {
        if (current()) setState({ identity, owner, reader: source.reader, values: Object.fromEntries(values) });
      })
      .catch((error) => {
        if (current()) setState({ identity, owner, reader: source.reader, error: String(error).slice(0, 512) });
      });
    return () => {
      mounted = false;
    };
  }, [identity, source?.reader]);
  if (state?.identity === identity && state.reader === source?.reader) return state;
  // Keep an already read body during a cut renewal only while its native
  // owner, exact selectors and reader still match. Never retain an old error.
  return state?.owner === owner && state.reader === source?.reader && state.values
    ? { ...state, error: undefined }
    : undefined;
}
