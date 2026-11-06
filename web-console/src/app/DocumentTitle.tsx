/**
 * Copyright (c) 2026 DeepSeek
 * SPDX-License-Identifier: MIT
 * Adapted from ui-layout/DocumentTitle.tsx; native rustX catalog owns titles.
 */
import { useEffect } from 'react';
import type { AppServerClient } from '../client/app-server';
import { useClientSelector } from '../client/selectors';

/** Only a durable title belongs in the browser tab; previews label list rows. */
export function DocumentTitle({ client, sessionId }: { client: AppServerClient; sessionId?: string }): null {
  const title = useClientSelector(client, state => {
    if (!sessionId) return;
    const session = state.sessions.find(row => row.id === sessionId) ?? state.views[sessionId]?.summary;
    return session?.name ?? session?.generated_title?.text ?? undefined;
  });
  useEffect(() => {
    document.title = title === undefined ? 'rustX' : `${title} — rustX`;
    return () => { document.title = 'rustX'; };
  }, [title]);
  return null;
}
