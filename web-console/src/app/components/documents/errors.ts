import { WorkspaceHostError } from '../../../workspaces/host';
import { settlementFailureKind } from '../../../workspaces/host';
const codes = ['too_large', 'capacity', 'parser_limit', 'malformed', 'encrypted', 'worker_failure', 'parser_failure', 'parser_timeout', 'preview_unavailable', 'converter_unavailable', 'converter_timeout', 'converter_failure', 'archive_rejected', 'source_changed', 'source_missing', 'source_unavailable', 'authorization_revoked', 'obsolete', 'failure'] as const;
export function documentFailure(cause: unknown): typeof codes[number] {
  const settlement = settlementFailureKind(cause);
  if (settlement) return settlement === 'converter_settlement_unknown' ? 'converter_unavailable' : 'preview_unavailable';
  if (cause instanceof WorkspaceHostError && cause.kind === 'capacity') return 'capacity';
  if (cause instanceof WorkspaceHostError && cause.native?.kind === 'session_file_read') {
    const reason = cause.native.reason;
    if (reason === 'unauthorized') return 'authorization_revoked';
    if (reason === 'missing') return 'source_missing';
    if (reason === 'unavailable' || reason === 'not_regular') return 'source_unavailable';
    if (reason === 'replaced') return 'source_changed';
    if (reason === 'too_large' || reason === 'capacity') return reason;
  }
  if (cause instanceof WorkspaceHostError && cause.kind === 'authority_replaced') return 'authorization_revoked';
  if (cause instanceof WorkspaceHostError && ['stale_attachment', 'stale_runtime', 'unknown_session', 'unknown_node'].includes(cause.kind ?? '')) return 'obsolete';
  if (cause instanceof WorkspaceHostError) { const code = codes.find(code => code === cause.kind); if (code) return code; }
  if (cause instanceof Error && cause.name === 'PasswordException') return 'encrypted';
  if (cause instanceof Error && ['InvalidPDFException', 'UnknownErrorException'].includes(cause.name)) return 'malformed';
  const message = cause instanceof Error ? cause.message : '';
  return codes.find(code => message === code || message.endsWith(`Error: ${code}`)) ?? 'failure';
}
