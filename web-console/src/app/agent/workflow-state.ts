import type { WorkflowState } from '../../../../protocol/app-server/v51';
import type { Translate } from '../../locale/translation';
import type { StateDotState } from '../../presentation/primitives/StateDot';

export function workflowDot(state: WorkflowState): StateDotState {
  return state.type !== 'settled' ? state.type === 'waiting' ? 'warning' : state.type === 'pending' ? 'idle' : 'ongoing'
    : state.outcome === 'completed' ? 'done' : state.outcome === 'failed' ? 'error' : 'warning';
}

export function workflowStatus(state: WorkflowState, tx: Translate): string {
  return state.type === 'waiting' ? tx('common:activity.waiting-for', { state: tx(`common:state.${state.reason}`) })
    : tx(`common:state.${state.type === 'settled' ? state.outcome : state.type}`);
}
