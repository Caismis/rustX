import { useSyncExternalStore } from 'react';
import type { AppServerClient } from '../../client/app-server';
import { useTranslation } from '../../locale/react';
import { Button } from '../../presentation/primitives/Button';

/** Retained input belongs to the old authority. Inspection/discard only; never
 * offer a replay or associate its Session ID with the replacement endpoint. */
export function DetachedFirstSubmissions({ client, binding, active }: { client: AppServerClient; binding: string; active: boolean }) {
  const tx = useTranslation();
  const owner = client.firstSubmissions;
  const retained = useSyncExternalStore(owner.subscribe, owner.detachedSnapshot);
  return retained.filter(value => owner.draft(value.binding) !== value || value.binding !== binding || active).map(value => <section className="notice" key={`${value.authority}:${value.binding}`}>
    <details><summary>{tx('common:startup.detached')}</summary>
      <pre>{value.draft.text}</pre>
      <ul>{value.draft.files.map((file, index) => <li key={index}>{file.name}</li>)}</ul>
      <p>{tx('common:startup.detached-receipts', { count: value.receipts.length })}</p>
      <Button disabled={!['rejected', 'failed', 'uncertain'].includes(value.phase)} onClick={() => owner.discard(value)}>{tx('common:conversation-composer.discard-first-submission-draft')}</Button>
    </details>
  </section>);
}
