/* Copyright (c) 2026 DeepSeek. MIT. Source port; see PROVENANCE.md. */
/** File-extension preview registrations; component dispatch belongs to the keyed document slot. */
import { documentFileName, matchedSuffixLength } from './suffix.ts'

/** Shared text or byte reads, or content loading owned by the renderer. */
export type DocumentLoadMode = 'text-pages' | 'bytes-complete' | 'renderer'

/** One renderer implementation, independent of its component registration. */
export interface DocumentPreviewDefinition {
  /** Unique implementation name, also used as the document slot key. */
  readonly id: string
  /** File suffixes without a leading dot; compound suffixes such as tar.gz are accepted. */
  readonly extensions: readonly string[]
  /**
   * Suffixes among `extensions` whose bytes are not readable text; a file
   * matching one loses the plain-text fallback among its viewer choices.
   * Every entry must appear in `extensions`; `register` rejects strays.
   */
  readonly binaryExtensions?: readonly string[]
  /** External implementations win over product implementations; defaults to extension. */
  readonly priority?: 'builtin' | 'extension'
  /** Localized implementation label, evaluated when the toolbar renders. @returns the visible name. */
  /** Content delivery mode supplied by the document owner. */
  readonly loading: DocumentLoadMode
  /** Whether the implementation consumes the document's wrap preference. */
  readonly wrap?: boolean
}

/**
 * Rank an observed definition snapshot without consulting mutable service state.
 * @param definitions - registered implementations in registration order.
 * @param path - decoded filename or file path.
 * @returns matching implementations, external band first, then longest suffix.
 */
export function matchingDocumentPreviews(
  definitions: readonly DocumentPreviewDefinition[],
  path: string,
): readonly DocumentPreviewDefinition[] {
  const name = documentFileName(path)
  return definitions.map((definition, order) => ({
    definition, order,
    rank: definition.priority === 'builtin' ? 0 : 1,
    length: matchedSuffixLength(name, definition.extensions),
  }))
    .filter(candidate => candidate.length > 0)
    .sort((left, right) => right.rank - left.rank || right.length - left.length || left.order - right.order)
    .map(candidate => candidate.definition)
}

/**
 * Whether any registered implementation declares the filename's suffix binary.
 * @param definitions - registered implementations.
 * @param path - decoded filename or file path.
 * @returns true when a declared binary suffix matches the filename.
 */
export function binaryDocumentPath(
  definitions: readonly DocumentPreviewDefinition[],
  path: string,
): boolean {
  const name = documentFileName(path)
  return definitions.some(definition => matchedSuffixLength(name, definition.binaryExtensions ?? []) > 0)
}
