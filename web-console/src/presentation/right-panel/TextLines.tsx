/* Copyright (c) 2026 DeepSeek. MIT. Adapted from TextBody; see PROVENANCE.md. */
import css from './TextLines.module.css';
/** Native text pages arrive as one retained prefix; keep stable source-line targets. */
export function TextLines({ text }: { text: string }) {
  const lines = text.split('\n');
  return <pre className={css.page} data-textpreview-plain>{lines.map((line, index) =>
    <div className={css.line} key={index} data-textpreview-line={index + 1}>{line}{index < lines.length - 1 ? '\n' : ''}</div>)}</pre>;
}
