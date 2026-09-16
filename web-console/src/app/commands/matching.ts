/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-primitives/rank-by-name.ts; see PROVENANCE.md. */
import { commands, type CommandDefinition } from './registry';
function boundary(name: string, index: number) {
  return index === 0 || /[-_ ]/.test(name[index - 1]) ? 8 : 0;
}
function score(name: string, query: string): number {
  if (query.length > name.length) return -Infinity;
  let previous = Array<number>(name.length).fill(-Infinity);
  for (let i = 0; i < name.length; i++) if (name[i] === query[0]) previous[i] = 1 + boundary(name, i) - i;
  for (let q = 1; q < query.length; q++) {
    const current = Array<number>(name.length).fill(-Infinity);
    let left = -Infinity, leftLeft = -Infinity, gapped = -Infinity;
    for (const [i, prior] of previous.entries()) {
      gapped = Math.max(gapped, leftLeft + i - 2);
      if (name[i] === query[q]) current[i] = 1 + boundary(name, i) + Math.max(left + 4, gapped + 1 - i);
      leftLeft = left; left = prior;
    }
    previous = current;
  }
  return Math.max(-Infinity, ...previous);
}
/** Prefix, strongest subsequence alignment, then registry order; never locale sort. */
export function matchCommands(raw: string, catalog: readonly CommandDefinition[] = commands) {
  const query = raw.toLowerCase();
  if (!query) return catalog;
  return catalog.map((command, index) => {
    const keys = [command.id, command.label, ...command.aliases].map(key => key.toLowerCase());
    return { command, index, prefix: keys.some(key => key.startsWith(query)), score: Math.max(...keys.map(key => score(key, query))) };
  }).filter(item => item.score !== -Infinity).sort((a, b) => Number(b.prefix) - Number(a.prefix) || b.score - a.score || a.index - b.index).map(item => item.command);
}
