import type { Translate } from '../locale/translation';
import type { SessionSummary } from '../../../protocol/app-server/v26';

/** Native catalog metadata, never a browser-generated title or protocol identity. */
export function sessionDisplayTitle(tx: Translate, summary?: Pick<SessionSummary, 'name' | 'preview'>): string {
  return summary?.name ?? summary?.preview ?? tx('agent:copy.new-session');
}
