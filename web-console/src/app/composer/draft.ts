import type { UserInputBlock } from '../../../../protocol/app-server/v46';

/** One resident editor, retained across a disposable read-only history surface.
 * This memory-only snapshot carries content, never operation/control authority. */
export interface ComposerDraft {
  snapshot?: {
    identity: string;
    text: string;
    restored: Extract<UserInputBlock, { type: 'upload' }>[];
    supported: boolean;
    receiptAuthority?: string;
    selection?: { start: number; end: number; direction: 'forward' | 'backward' | 'none' };
  };
}
