/* Copyright (c) 2026 DeepSeek. MIT. Adapted from TextBody; see PROVENANCE.md. */
/** Native text pages arrive as one retained prefix; keep stable source-line targets. */
export function TextLines({ text }: { text: string }) {
  const lines = text.split('\n');
  return <pre className="m-0 [font:var(--dsw-font-markdown-code-block)] [white-space:inherit]" data-textpreview-plain>{lines.map((line, index) =>
    <div className="min-h-[1lh]" key={index} data-textpreview-line={index + 1}>{line}{index < lines.length - 1 ? '\n' : ''}</div>)}</pre>;
}
