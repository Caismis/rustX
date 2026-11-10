import { useState, type ComponentProps } from 'react';
import { Menu } from '../primitives/Menu';
import { IconEllipsisOutline16 } from '../primitives/icons';
import css from './Catalog.module.css';

export type CardMenuItems = ComponentProps<typeof Menu>['items'];

export function CardMenu({ label, items, onSelect }: { label: string; items: CardMenuItems; onSelect: (id: string) => void }) {
  const [open, setOpen] = useState(false);
  return <Menu open={open} onClose={() => setOpen(false)} align="end" autoFocus items={items} onSelect={id => { setOpen(false); onSelect(id); }}
    anchor={<button type="button" className={css.dots} aria-label={label} aria-haspopup="menu" aria-expanded={open}
      onClick={() => setOpen(value => !value)}><IconEllipsisOutline16 /></button>} />;
}
