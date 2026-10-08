import { IconPaperclipOutline16, IconDataOutline16, IconCompactOutline16, IconPlusOutline16, IconBranchOutline16, IconGoalOutline16 } from '../../presentation/primitives/icons';
import type { MenuAction } from './CommandMenu';
// git-fork and wrench: Lucide 0.468.0, ISC. See public/LICENSE-Lucide.txt.
export function CommandIcon({ id }: { id: MenuAction }) {
  const Icon = { file: IconPaperclipOutline16, model: IconDataOutline16, compact: IconCompactOutline16, new: IconPlusOutline16, branch: IconBranchOutline16, goal: IconGoalOutline16 }[id as Exclude<MenuAction, 'fork' | 'tools'>];
  if (Icon) return <Icon />;
  return <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    {id === 'fork' ? <><circle cx="12" cy="18" r="3"/><circle cx="6" cy="6" r="3"/><circle cx="18" cy="6" r="3"/><path d="M18 9v2c0 .6-.4 1-1 1H7c-.6 0-1-.4-1-1V9M12 12v3"/></> : <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/>}
  </svg>;
}
