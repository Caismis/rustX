import { useContext, useEffect, useRef, useState } from 'react';
import { useSelector } from '@xstate/react';
import type { WorkflowAnalysisQuery, WorkflowAnalysisReport } from '../../../../../../../protocol/app-server/v62';
import { WorkflowAnalysisSession, type AnalysisInput } from '../../../document/analysis';
import { targetKey } from '../../../port';
import { DraftOwner } from './drafts';

/** The source target, native candidate, browser generation, consumer and query
 * are separate correlation facts. A stale response is never rendered for a new
 * field, even in the render before effect cleanup. */
export function useCandidateAnalysis(query?: WorkflowAnalysisQuery, active = true) {
  const owner = useContext(DraftOwner);
  if (!owner) throw new Error('Candidate analysis needs its editor');
  const context = useSelector(owner.editor, snapshot => snapshot.context);
  const identity = JSON.stringify([targetKey(context.target), context.generation, query]);
  const input: AnalysisInput | undefined = active && context.document ? { port: context.port, target: context.target, document: context.document, generation: context.generation, query } : undefined;
  const current = useRef(input); current.current = input;
  const session = useRef(new WorkflowAnalysisSession());
  const [result, setResult] = useState<{ identity: string; port: AnalysisInput['port']; document: AnalysisInput['document']; report?: WorkflowAnalysisReport; failed?: boolean }>();
  useEffect(() => {
    const input = current.current, request = session.current;
    if (input) void request.request(input, () => current.current).then(report => { if (report) setResult({ identity, port: input.port, document: input.document, report }); }, () => {
      if (current.current?.document === input.document && current.current.port === input.port && current.current.generation === input.generation) setResult({ identity, port: input.port, document: input.document, failed: true });
    });
    return () => request.cancel();
  }, [identity, active, context.port]);
  return result?.identity === identity && result.port === context.port && result.document === context.document && active ? result : undefined;
}
