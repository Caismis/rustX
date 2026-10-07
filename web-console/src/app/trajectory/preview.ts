/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness ui-trajectory/trajectory-preview.ts and ui-primitives/markdown/plain-text.ts; see PROVENANCE.md. */
/**
 * Bounded Markdown-to-text projection for one-line ledger rows.
 *
 * Parsing shares the renderer's GFM grammar, so the projection strips exactly
 * the markup the renderer would draw; raw HTML stays literal, links keep
 * their labels, images keep alt text and code keeps its source text.
 */
import { parseGfm } from '../../presentation/markdown/parse';

const PREVIEW_SOURCE_CHARACTERS = 2_048;
const PREVIEW_OUTPUT_CHARACTERS = 512;

interface MarkdownNode {
  type: string;
  value?: string;
  alt?: string | null;
  children?: MarkdownNode[];
}

function inlineText(node: MarkdownNode): string {
  switch (node.type) {
    case 'text':
    case 'inlineCode':
    case 'code':
    case 'html':
      return node.value ?? '';
    case 'image':
    case 'imageReference':
      return node.alt ?? '';
    case 'break':
      return '\n';
    default:
      return node.children?.map(inlineText).join('') ?? '';
  }
}

const compactInline = (text: string) => text.replace(/\s+/g, ' ').trim();

function blockText(node: MarkdownNode): string {
  switch (node.type) {
    case 'root':
    case 'blockquote':
      return node.children?.map(blockText).filter(Boolean).join('\n\n') ?? '';
    case 'code':
      return node.value?.trim() ?? '';
    case 'list':
    case 'table':
      return node.children?.map(blockText).filter(Boolean).join('\n') ?? '';
    case 'listItem':
      return node.children?.map(blockText).filter(Boolean).join(' ') ?? '';
    case 'tableRow':
      return node.children?.map(blockText).join('\t') ?? '';
    case 'html':
      return node.value ?? '';
    case 'thematicBreak':
    case 'definition':
      return '';
    default:
      return compactInline(inlineText(node));
  }
}

/**
 * Build a bounded one-line preview without parsing the complete Markdown document.
 * @param text - Untrusted message, reasoning, payload, or result text.
 * @returns A compact preview capped independently from the retained source.
 */
export function trajectoryPreviewText(text: string): string {
  const source = text.slice(0, PREVIEW_SOURCE_CHARACTERS);
  const compact = blockText(parseGfm(source) as MarkdownNode).replace(/\s+/g, ' ').trim();
  const preview = compact.slice(0, PREVIEW_OUTPUT_CHARACTERS).trimEnd();
  return source.length < text.length || preview.length < compact.length ? `${preview}…` : preview;
}
