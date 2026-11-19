/** Keep shell rail and sidebar glyphs synchronous without bundling the complete icon set.
 * Re-run with `node scripts/sidebar-icons.ts` after upgrading the Lucide set
 * or changing the selection below. License: licenses/iconify-json-lucide-1.2.144.txt. */
import { writeFileSync } from 'node:fs';
import lucide from '@iconify-json/lucide/icons.json' with { type: 'json' };

const names = ['house', 'workflow', 'bot', 'braces', 'clock', 'settings', 'panel-left-close', 'square-pen'] as const;
const icons = Object.fromEntries(names.map(name => [name, {
  width: lucide.width, height: lucide.height, ...lucide.icons[name],
}]));

writeFileSync(new URL('../src/presentation/sidebar/sidebar-icons.json', import.meta.url), JSON.stringify(icons, null, 2) + '\n');
