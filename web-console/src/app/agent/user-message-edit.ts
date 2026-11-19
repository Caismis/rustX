import { useEffect, useRef, useState } from 'react';
import type { CanonicalContentLocator, SessionMessageVersion, SessionMessageVersionFamily } from '../../../../protocol/app-server/v82';
import { canonicalLocatorKey } from '../../../../protocol/app-server/content-identity';
import { readMessageBlocks } from './message-blocks';
import { observationSourceIdentity, useObservationSource } from './ObservationSource';

export interface UserMessageHistory {
  versions: ReadonlyMap<string, SessionMessageVersionFamily>;
  retry: (locator: CanonicalContentLocator, replacementText?: string) => void;
  switchVersion: (version: SessionMessageVersion) => void;
}

type Edit = {
  key: string;
  owner: string;
  phase: 'loading' | 'failed';
} | {
  key: string;
  owner: string;
  phase: 'ready';
  original: string;
  text: string;
  hasAttachments: boolean;
};

/** One editor per Conversation. Deferred body bytes come from the selected native message. */
export function useUserMessageEdit(disabled: boolean) {
  const source = useObservationSource();
  const owner = JSON.stringify(observationSourceIdentity(source) ?? null);
  const [state, setState] = useState<Edit>();
  const pending = useRef<AbortController>(null);
  const edit = state?.owner === owner ? state : undefined;
  useEffect(() => () => pending.current?.abort(), [owner]);

  const open = async (locator: CanonicalContentLocator) => {
    if (disabled || !source?.owned()) return;
    pending.current?.abort();
    const operation = new AbortController();
    pending.current = operation;
    const key = canonicalLocatorKey(locator);
    const current = () => !operation.signal.aborted && source.owned();
    setState({ key, owner, phase: 'loading' });
    try {
      const blocks = await readMessageBlocks(source, { locator, offset: 0 }, current);
      if (!current()) return;
      if (!blocks.length || blocks.some(block => block.type !== 'text' && block.type !== 'uploaded_file')) {
        throw new Error('The original message is outside the editable input types.');
      }
      const original = blocks.flatMap(block => block.type === 'text' ? [block.text] : []).join('\n\n');
      setState({ key, owner, phase: 'ready', text: original, original,
        hasAttachments: blocks.some(block => block.type === 'uploaded_file') });
    } catch {
      if (current()) setState({ key, owner, phase: 'failed' });
    } finally {
      if (pending.current === operation) pending.current = null;
    }
  };

  return {
    edit,
    open,
    setText: (text: string) => setState(value => value?.owner === owner && value.phase === 'ready' ? { ...value, text } : value),
    cancel: () => { pending.current?.abort(); setState(undefined); },
  };
}
