/**
 * Deterministic render helpers for the presentation suites.
 *
 * Rendering is proven without a terminal, which is the point: Pi sits at the
 * outermost layer and every correctness question below it is answerable from
 * strings.
 */

import { replaceFromSnapshot } from "../../src/presentation/projection.ts";
import type { PresentationState } from "../../src/presentation/state.ts";
import type { RuntimeClientSnapshot } from "../../src/protocol/types.ts";
import {
  type TranscriptBlock,
  renderTranscript,
} from "../../src/ui/components/transcript.ts";
import {
  type PresentationPreferences,
  defaultPreferences,
} from "../../src/ui/preferences.ts";
import { plainText } from "../../src/ui/theme.ts";
import { runtimeCursor, snapshot } from "./fixtures.ts";

export { plainText as plain };

/** A presentation state built from one snapshot. */
export function stateOf(
  overrides: Partial<RuntimeClientSnapshot> = {},
): PresentationState {
  return replaceFromSnapshot(snapshot(overrides), runtimeCursor(0));
}

export function prefs(
  overrides: Partial<PresentationPreferences> = {},
): PresentationPreferences {
  return { ...defaultPreferences(), ...overrides };
}

/** The plain text of one rendered block. */
export function blockText(block: TranscriptBlock): string {
  return plainText(block.kind === "markdown" ? block.markdown : block.text);
}

/** The whole transcript as plain text blocks. */
export function transcriptText(
  state: PresentationState,
  preferences: PresentationPreferences = defaultPreferences(),
): string[] {
  return renderTranscript(state, preferences).map(blockText);
}

/** The whole transcript as one plain string. */
export function transcriptString(
  state: PresentationState,
  preferences: PresentationPreferences = defaultPreferences(),
): string {
  return transcriptText(state, preferences).join("\n---\n");
}
