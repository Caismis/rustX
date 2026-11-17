import { useRef, useState, type ComponentProps } from 'react';
import { Menu } from '../primitives/Menu';
import { IconEllipsisOutline16 } from '../primitives/icons';
import css from './Catalog.module.css';

export type CardMenuItems = ComponentProps<typeof Menu>['items'];

export function CardMenu({ label, items, onSelect }: { label: string; items: CardMenuItems; onSelect: (id: string) => void }) {
  const [open, setOpen] = useState(false);
  // A catalog refresh rebuilds these rows and can disable or replace them
  // while the pointer is already over the open menu. Keep the opened rows
  // and their selection closure until the menu closes, so click, keyboard,
  // and release-outside still address the same controls.
  const opened = useRef({ items, onSelect });
  if (!open) opened.current = { items, onSelect };
  const shown = open ? opened.current : { items, onSelect };
  return <Menu open={open} onClose={() => setOpen(false)} align="end" autoFocus items={shown.items}
    onSelect={id => { setOpen(false); shown.onSelect(id); }}
    anchor={<button type="button" className={css.dots} aria-label={label} aria-haspopup="menu" aria-expanded={open}
      onClick={() => {
        if (!open) opened.current = { items, onSelect };
        setOpen(value => !value);
      }}><IconEllipsisOutline16 /></button>} />;
}
