/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-conversation ConversationRoot composer
// context stack. Order is fixed by construction; no slot registry or plugin order.
import type { ReactNode } from 'react';

/** The parent owns order, shared width and rhythm. Each dock owns only its own
 * presentation state, so one appearing or disappearing transfers nothing. */
export function ComposerContextStack({ todo, goal, queue, composer }: {
  todo: ReactNode; goal: ReactNode; queue: ReactNode; composer: ReactNode;
}) {
  return <div className="[--dsh-composer-dock-inset:8px] [--dsh-composer-stack-gap:6px] flex flex-none flex-col gap-[var(--dsh-composer-stack-gap)] min-w-0 pt-[var(--dsh-composer-stack-gap)]" data-composer-context-stack="">{todo}{goal}{queue}{composer}</div>;
}
