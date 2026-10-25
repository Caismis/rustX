import type { Response } from '../../../protocol/app-server/v57';

/** Request outcome contracts of one App Server connection. They sit below the
 * client and the application flows that classify its failures, so neither
 * depends on the other to recognise an unsent, refused or uncertain request. */
/** The request owner proves that transport dispatch never began. */
export class RequestNotDispatched extends Error {
  constructor(cause: unknown) {
    super(cause instanceof Error ? cause.message : String(cause), { cause });
  }
}
/** Capacity refusal carries the exact readiness cut at which nothing was sent. */
export class RequestAdmissionDeferred extends RequestNotDispatched {
  constructor(readonly revision: number) { super('Client request capacity reached.'); }
}
export class OutcomeUncertain extends Error {
  constructor() { super('Response lost after transmission. Outcome uncertain; the request was not replayed. Reconnect and inspect authoritative state.'); }
}
export function isOutcomeUncertain(error: unknown): boolean {
  return error instanceof OutcomeUncertain || (error instanceof RpcFailure && ["committed_durability_uncertain", "outcome_unknown"].includes(error.error.data?.kind ?? ""));
}
export class RpcFailure extends Error {
  constructor(readonly error: Extract<Response, { error: unknown }>['error']) {
    const data = error.data;
    let message = `${error.message} (${error.code})${data ? `: ${JSON.stringify(data)}` : ''}`;
    switch (data?.kind) {
      case 'agent_not_delivered':
        message = `Agent ${data.agent_id} input was not delivered`;
        break;
      case 'agent_delivery_unknown':
        message = `Agent ${data.agent_id} input acceptance was not acknowledged; delivery is unknown, do not replay automatically`;
        break;
      case 'job_publication_abandoned':
        message = `Job ${data.job_id} terminal publication was abandoned; no durable terminal result is available`;
        break;
      case 'agent_settlement':
        message = `Agent ${data.agent_id} is unavailable; physical settlement, publication, or workspace authority remains unresolved`;
        break;
      case 'archive_preparation_failed':
        message = error.message;
    }
    super(message);
  }
}
