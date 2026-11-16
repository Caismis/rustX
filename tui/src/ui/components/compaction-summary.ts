/**
 * Pi Coding Agent compaction-message presentation, adapted to native checkpoints.
 * Reference: earendil-works/pi@850ea2eae2f39eb16ae14e1f09448eab6da2188a,
 * packages/coding-agent/src/modes/interactive/components/compaction-summary-message.ts.
 * Copyright (c) 2025 Mario Zechner. MIT; see THIRD_PARTY_NOTICES.md.
 */
import { Box, Markdown, Spacer, Text } from '@earendil-works/pi-tui';
import type { RuntimeClientCompactionView, RuntimeClientProjectionHeadView } from '../../protocol/app-server.ts';
import { groupedQuantity } from '../../protocol/quantities.ts';
import { sanitizeField } from '../../sanitize.ts';
import { background, markdownTheme, role, style } from '../theme.ts';

/** Expansion is a view preference; only a committed native head supplies content. */
export class CompactionSummary extends Box {
  constructor(head: RuntimeClientProjectionHeadView, expanded: boolean, receipt?: RuntimeClientCompactionView) {
    super(1, 1, background.compaction);
    this.addChild(new Text(style.bold(role.compactionLabel('[compaction]')), 0, 0));
    this.addChild(new Spacer(1));
    const before = receipt?.checkpoint_id === head.checkpoint_id ? receipt.tokens_before : undefined;
    const label = before ? `Compacted from ${before.source === 'estimated' ? '~' : ''}${groupedQuantity(before.input_tokens)} tokens` : 'Context compacted';
    const preview = head.summary_preview;
    if (!preview) {
      this.addChild(new Text(role.compactionText(`${label} · Summary unavailable`), 0, 0));
    } else if (expanded) {
      this.addChild(new Markdown(`**${label}**\n\n${sanitizeField(preview.text, true)}`, 0, 0, markdownTheme));
      if (preview.truncated) this.addChild(new Text(role.meta('Bounded summary preview · /context reads the full checkpoint'), 0, 0));
      this.addChild(new Text(role.meta('/expand compaction to collapse'), 0, 0));
    } else {
      this.addChild(new Text(role.compactionText(`${label} (/expand compaction to expand)`), 0, 0));
    }
  }
}
