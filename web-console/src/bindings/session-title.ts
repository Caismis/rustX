import type { SessionSummary } from '../../../protocol/app-server/v25';

/** Native catalog metadata, never a browser-generated title or protocol identity. */
export function sessionDisplayTitle(summary?: Pick<SessionSummary, 'name' | 'preview'>): string {
  return summary?.name ?? summary?.preview ?? 'New session';
}
