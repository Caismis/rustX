/** Bounded background observations. No execution or durable work is queued here. */
export function meterQueue() {
  const active = new Set<string>();
  const pending = new Map<string, () => Promise<void>>();
  const drain = () => {
    for (const [key, task] of pending) {
      if (active.size >= 2) break;
      if (active.has(key)) continue;
      pending.delete(key);
      active.add(key);
      void Promise.resolve().then(task).finally(() => { active.delete(key); drain(); });
    }
  };
  return (key: string, task: () => Promise<void>) => {
    // At most one latest requirement per identity. At capacity the oldest
    // undispatched observation is discarded; unavailable meters stay unknown.
    pending.delete(key);
    if (pending.size >= 32) pending.delete(pending.keys().next().value!);
    pending.set(key, task);
    drain();
    return () => { if (pending.get(key) === task) pending.delete(key); };
  };
}
