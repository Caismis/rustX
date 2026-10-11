import type { RuntimeClientJob, TraceState } from '../../../protocol/app-server/v44';
import type { Translate, TranslationKey } from '../locale/translation';

// Native states remain the keys. These exhaustive maps own only product labels.
const jobStates = {
  starting: 'common:state.starting', running: 'common:state.running',
  cancelling: 'common:state.cancelling', publishing_terminal: 'common:state.publishing_terminal',
  succeeded: 'common:state.succeeded', failed: 'common:state.failed', denied: 'common:state.denied',
  cancelled: 'common:state.cancelled', timed_out: 'common:state.timed_out', outcome_unknown: 'common:state.outcome_unknown',
} satisfies Record<RuntimeClientJob['state'], TranslationKey>;
const traceStates = {
  incomplete: 'common:state.incomplete', running: 'common:state.running', pending: 'common:state.pending',
  cancelling: 'common:state.cancelling', settling: 'common:state.settling', waiting: 'common:state.waiting',
  completed: 'common:state.settled', failed: 'common:state.failed', cancelled: 'common:state.cancelled',
  timed_out: 'common:state.timed_out', limited: 'common:state.limited', denied: 'common:state.denied',
  outcome_unknown: 'common:state.outcome_unknown', interrupted: 'common:state.interrupted',
} satisfies Record<TraceState, TranslationKey>;
export const jobStateLabel = (tx: Translate, state: RuntimeClientJob['state']) => tx(jobStates[state]);
export const traceStateLabel = (tx: Translate, state: TraceState) => tx(traceStates[state]);
