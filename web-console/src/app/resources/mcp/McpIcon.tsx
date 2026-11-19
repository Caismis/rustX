import mcpUrl from '@lobehub/icons-static-svg/icons/mcp.svg?url';
import css from './McpIcon.module.css';

/** LobeHub's MCP brand mark as a monochrome mask; theme and state ride currentColor. */
export function McpIcon({ size = 16 }: { size?: number }) {
  return <span aria-hidden="true" className={css.glyph} style={{ width: size, height: size, maskImage: `url("${mcpUrl}")` }} />;
}
