import { RpcFailure } from '../client/errors';

/** Keep the native message/code, without dumping wire JSON into a notice.
 * The original Error and native diagnostic owner retain structured evidence. */
export function actionError(error: unknown): string {
  if (error instanceof RpcFailure) {
    const data = error.error.data;
    const diagnostic = data && 'diagnostic' in data ? data.diagnostic : undefined;
    const code = data && 'reason' in data && typeof data.reason === 'string' ? `${data.kind}/${data.reason}` : data?.kind ?? error.error.code;
    return `${diagnostic || error.error.message} (${code})`;
  }
  return error instanceof Error ? error.message : String(error);
}
