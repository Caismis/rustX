import type { Translate } from '../locale/translation';
import type { SessionSummary } from '../../../protocol/app-server/v57';

/** Native catalog metadata followed by the exact Session identity. */
export function sessionDisplayTitle(tx: Translate, summary?: Partial<Pick<SessionSummary, 'id' | 'name' | 'generated_title' | 'preview'>>): string {
  return summary?.name ?? summary?.generated_title?.text ?? summary?.preview ?? summary?.id ?? tx('agent:copy.new-session');
}
