import { useEffect, useRef, useState } from 'react';
import { findNodeAtLocation, getNodeValue, parseTree, type ParseError } from 'jsonc-parser';
import type { ForegroundToolExecution, ScheduleHeader, ScheduleSpec } from '../../../../protocol/app-server/v62';
import { ContentCache } from '../../../../protocol/app-server/content-cache';
import { canonicalLocatorKey } from '../../../../protocol/app-server/content-identity';
import { readContentText } from '../../client/content-text';
import { useContentSource, observationSourceIdentity } from '../agent/ObservationSource';
import { readToolOccurrence } from '../agent/tool-source';
import { Button } from '../../presentation/primitives/Button';
import { IconClockOutline16 } from '../../presentation/primitives/icons';
import { useLocale } from '../../locale/react';
import { useTaskCopy } from './copy';
import { useTaskNavigation } from './SessionTasks';
import { taskRef, useTaskCatalog } from './model';
import { frequency } from './timing';
import css from './scheduler.module.css';

/** Native Tool JSON is opaque to the wire codec. Preserve decimal identities
 * from its original UTF-8 instead of rounding them through JavaScript numbers. */
export function createdReceipt(raw: string): { id: ScheduleHeader['id']; spec: ScheduleSpec } | undefined {
  const errors: ParseError[] = [], tree = parseTree(raw, errors, { disallowComments: true });
  if (!tree || errors.length || findNodeAtLocation(tree, ['status'])?.value !== 'applied') return;
  const conversation = findNodeAtLocation(tree, ['receipt', 'id', 'conversation_id']);
  const ordinal = findNodeAtLocation(tree, ['receipt', 'id', 'ordinal']);
  const spec = findNodeAtLocation(tree, ['receipt', 'spec']);
  if (conversation?.type !== 'string' || !ordinal || !spec) return;
  const decimal = ordinal.type === 'string' ? String(ordinal.value) : raw.slice(ordinal.offset, ordinal.offset + ordinal.length);
  if (!/^(0|[1-9][0-9]*)$/.test(decimal) || BigInt(decimal) > 18_446_744_073_709_551_615n) return;
  const value: unknown = getNodeValue(spec);
  if (!value || typeof value !== 'object' || !('type' in value)
    || !['at', 'interval', 'cron'].includes(String(value.type))) return;
  return { id: { conversation_id: conversation.value, ordinal: decimal }, spec: value as ScheduleSpec };
}

export function CreatedTasks({ tools }: { tools: readonly ForegroundToolExecution[] }) {
  const { source } = useContentSource(), navigation = useTaskNavigation();
  if (!source || !navigation || 'target' in source && source.agentId) return null;
  const unique = new Map(tools.filter(tool => tool.tool_id === 'native.schedule_command'
    && (tool.committed_result?.status.type === 'success' || tool.committed_result_deferred))
    .map(tool => [canonicalLocatorKey(tool.occurrence.locator) + ':' + tool.block_index, tool]));
  return <>{[...unique].map(([key, tool]) => <CreatedTask key={key} tool={tool}/>)}</>;
}
function CreatedTask({ tool }: { tool: ForegroundToolExecution }) {
  const { source, read } = useContentSource(), navigation = useTaskNavigation();
  const t = useTaskCopy(), locale = useLocale().active;
  const { model, records } = useTaskCatalog(source!.client);
  const session = source && ('target' in source ? source.target.session_id : source.sessionId);
  const [created, setCreated] = useState<{ identity: string; title: string; receipt: NonNullable<ReturnType<typeof createdReceipt>> }>();
  const identity = JSON.stringify([observationSourceIdentity(source), tool.occurrence, tool.result_metadata_ref, tool.committed_result_deferred, tool.state.arguments]);
  const latest = useRef(identity); latest.current = identity;
  useEffect(() => {
    if (!source?.owned()) return;
    let mounted = true;
    const current = () => mounted && latest.current === identity && source.owned();
    const cache = new ContentCache((ref, offset, _max, owned) => read(ref, offset, owned));
    void (async () => {
      const original = await readToolOccurrence(source, tool.occurrence.locator, tool.block_index, current, !!source.historyFrozen);
      if (original.tool.tool_id !== 'native.schedule_command' || original.tool.committed_result?.status.type !== 'success') return;
      const input = original.content.find(binding => binding.path.join('.') === 'state.arguments');
      const args = original.tool.state.arguments || (input ? await readContentText(cache, input.content_ref, current) : '');
      const mutation: unknown = JSON.parse(args).mutation;
      if (!mutation || typeof mutation !== 'object' || !('type' in mutation)
        || !['create', 'create_workflow'].includes(String(mutation.type))) return;
      const binding = original.content.find(binding => binding.path.join('.') === 'committed_result.content.0.value');
      const inline = original.tool.committed_result.content?.find(block => block.type === 'json');
      // A missing original payload may only use a provably exact small number.
      const value = inline?.type === 'json' ? inline.value : undefined;
      const ordinal = value && (value.receipt as { id?: { ordinal?: unknown } } | undefined)?.id?.ordinal;
      const raw = binding ? await readContentText(cache, binding.content_ref, current)
        : typeof ordinal === 'string' || typeof ordinal === 'number' && Number.isSafeInteger(ordinal) ? JSON.stringify(value) : '';
      const receipt = createdReceipt(raw);
      if (receipt && current()) setCreated({ identity, receipt, title: 'title' in mutation && typeof mutation.title === 'string' ? mutation.title.trim() : '' });
    })().catch(() => { /* A missing canonical result never becomes a creation claim. */ });
    return () => { mounted = false; cache.clear(); };
  }, [identity, read, source?.client]);
  const shown = created?.identity === identity ? created : undefined;
  useEffect(() => shown && session ? model.watchSession(session) : undefined, [model, session, !!shown]);
  if (!shown) return null;
  const record = records.find(row => row.binding.session_id === session && row.id.conversation_id === shown.receipt.id.conversation_id && row.id.ordinal === shown.receipt.id.ordinal);
  return <article className={css.created} aria-label={t('task.created')}><IconClockOutline16/>
    <div><strong>{record?.title || shown.title || t('task.created')}</strong><small>{frequency(record?.spec ?? shown.receipt.spec, t, locale)}</small></div>
    <Button size="sm" disabled={!record} onClick={() => record && navigation?.openTask(taskRef(record), record.title)}>{t('task.open')}</Button>
  </article>;
}
