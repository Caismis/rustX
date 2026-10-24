import type { RuntimeClientJob } from '../../../../protocol/app-server/v57';
import type { StateDotState } from '../../presentation/primitives/StateDot';

/** Publication is still owned work; an unknown terminal outcome is ended. */
export function jobActive(job: RuntimeClientJob): boolean {
  return job.state === 'starting' || job.state === 'running' || job.state === 'cancelling' || job.state === 'publishing_terminal';
}

export function jobDot(state: RuntimeClientJob['state']): StateDotState {
  if (state === 'succeeded') return 'done';
  if (state === 'cancelled' || state === 'outcome_unknown' || state === 'cancelling' || state === 'denied') return 'warning';
  if (state === 'failed' || state === 'timed_out') return 'error';
  return 'ongoing';
}
