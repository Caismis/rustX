/** Keep node glyphs synchronous without bundling the complete icon picker.
 * Re-run with `node scripts/workflow-icons.ts` after upgrading the Lucide set
 * or changing the selection below. License: licenses/iconify-json-lucide-1.2.144.txt. */
import { writeFileSync } from 'node:fs';
import lucide from '@iconify-json/lucide/icons.json' with { type: 'json' };

const names = ['log-in', 'bot', 'brain-circuit', 'wrench', 'git-branch', 'message-square-more',
  'repeat-2', 'list-restart', 'circle-stop', 'variable', 'log-out', 'house', 'braces', 'list-filter', 'tags', 'combine', 'square-terminal'] as const;
const icons = Object.fromEntries(names.map(name => [name, {
  width: lucide.width, height: lucide.height, ...lucide.icons[name],
}]));

writeFileSync(new URL('../src/app/workflows/ui/node-icons.json', import.meta.url), JSON.stringify(icons, null, 2) + '\n');
