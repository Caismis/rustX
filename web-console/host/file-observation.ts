/** Filesystem events are hints, never authority or content. The caller provides
 * a native delivery probe which owns descriptor and permission validation. */
import { watch, fstatSync, type FSWatcher } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { withWorkspacePath } from './workbench.ts';
import type { PreviewObservation } from '../src/client/preview-content.ts';

interface Edge { path: string; child: string; identity: string }
function edges(target: string): Edge[] {
  const parts = target.split('/').filter(Boolean);
  if (!target.startsWith('/') || parts.length > 128 || parts.some(part => part === '.' || part === '..' || part.includes('\0'))) throw new Error('Invalid observed file');
  const result: Edge[] = []; let parent = '/';
  for (const child of parts) {
    try {
      const identity = withWorkspacePath('/', parent.slice(1), true, fd => { const stat = fstatSync(fd, { bigint: true }); return `${stat.dev}:${stat.ino}`; });
      result.push({ path: parent, child, identity }); parent = join(parent, child);
    } catch { break; } // Closest existing ancestor observes later recreation.
  }
  if (!result.length) throw new Error('Observed file ancestry unavailable');
  return result;
}
function sameEdges(left: Edge[], right: Edge[]) { return left.length === right.length && left.every((value, i) => value.path === right[i].path && value.child === right[i].child && value.identity === right[i].identity); }

export async function observeFile(target: string, probe: () => Promise<string>, changed: () => void, signal: AbortSignal): Promise<PreviewObservation> {
  signal.throwIfAborted();
  let watchers: FSWatcher[] = [], current: Edge[] = [], closing = false, rearm: Promise<void> | undefined, pending = false;
  let finish!: () => void;
  const done = new Promise<void>(resolve => { finish = resolve; });
  const closeWatchers = async () => {
    const retired = watchers; watchers = []; current = [];
    await Promise.all(retired.map(watcher => new Promise<void>(resolve => { watcher.once('close', resolve); watcher.close(); })));
  };
  const close = async () => {
    if (closing) return done;
    closing = true; signal.removeEventListener('abort', abort);
    await rearm?.catch(() => {}); await closeWatchers(); finish();
  };
  const abort = () => { void close(); };
  const hint = () => {
    if (closing) return;
    // Invalidate synchronously, before any rearm/stat/read can wait.
    changed(); pending = true;
    if (rearm) return;
    rearm = (async () => {
      while (pending && !closing) {
        pending = false;
        const next = edges(target);
        if (sameEdges(current, next)) continue;
        await closeWatchers();
        if (closing) return;
        install(next);
        // Covers writes in the rearm gap; the consumer deduplicates versions.
        changed();
      }
    })().catch(() => { void close(); }).finally(() => { rearm = undefined; });
  };
  const install = (next: Edge[]) => {
    for (const edge of next) {
      const watcher = watch(edge.path, { persistent: false, encoding: 'buffer' }, (_event, name) => {
        if (name === null || name.equals(Buffer.from(edge.child))) hint();
      });
      watchers.push(watcher); watcher.on('error', () => { void close(); });
    }
    // fs.watch paths grant no access. Verify that every installed parent is
    // still the descriptor-walked directory before accepting the subscription.
    if (!sameEdges(next, edges(target))) throw new Error('Observed ancestry changed during admission');
    current = next;
  };
  signal.addEventListener('abort', abort, { once: true });
  try {
    install(edges(join(dirname(target), basename(target))));
    // Watch readiness precedes this native snapshot. Queued hints remain live.
    const version = await probe(); signal.throwIfAborted();
    if (closing) throw new Error('File observation unavailable');
    return { version, close, done };
  } catch (error) { await close(); throw error; }
}
