import { WorkspaceHostError, fileSourceFailure } from '../../../workspaces/host';
import { settlementFailureKind } from '../../../workspaces/host';
const codes = ['too_large', 'capacity', 'parser_limit', 'malformed', 'encrypted', 'worker_failure', 'parser_failure', 'parser_timeout', 'preview_unavailable', 'converter_unavailable', 'converter_timeout', 'converter_failure', 'archive_rejected', 'source_changed', 'source_missing', 'source_unavailable', 'authorization_revoked', 'obsolete', 'failure'] as const;
export function documentFailure(cause: unknown): typeof codes[number] {
  const settlement = settlementFailureKind(cause);
  if (settlement) return settlement === 'converter_settlement_unknown' ? 'converter_unavailable' : 'preview_unavailable';
  const source = fileSourceFailure(cause); if (source) return source;
  if (cause instanceof WorkspaceHostError) { const code = codes.find(code => code === cause.kind); if (code) return code; }
  if (cause instanceof Error && cause.name === 'PasswordException') return 'encrypted';
  if (cause instanceof Error && ['InvalidPDFException', 'UnknownErrorException'].includes(cause.name)) return 'malformed';
  const message = cause instanceof Error ? cause.message : '';
  return codes.find(code => message === code || message.endsWith(`Error: ${code}`)) ?? 'failure';
}
