import { MarkdownText as Markdown } from '../src/presentation/markdown/MarkdownText';
const defaults = { copyLabel: 'Copy', copiedLabel: 'Copied' };
const labels = { code: defaults, footnotes: 'Footnotes' };
export function MarkdownText({ codeLabels, ...props }: { text: string; streaming?: boolean; codeLabels?: typeof defaults }) {
  return <Markdown {...props} labels={codeLabels ? { code: codeLabels, footnotes: 'Footnotes' } : labels} />;
}
