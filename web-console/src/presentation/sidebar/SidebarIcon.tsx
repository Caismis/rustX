import { Icon } from '@iconify/react/offline';
import icons from './sidebar-icons.json';

/** Lucide names generated into sidebar-icons.json by scripts/sidebar-icons.ts. */
export type SidebarIconName = keyof typeof icons;

/** Primary rail and Home sidebar glyphs ride currentColor; the checked-in
 * subset is inline data, so the offline renderer needs no fetch or loading state. */
export function SidebarIcon({ name, size = 18, className }: { name: SidebarIconName; size?: number; className?: string }) {
  return <Icon icon={icons[name]} width={size} height={size} className={className} aria-hidden="true" />;
}
