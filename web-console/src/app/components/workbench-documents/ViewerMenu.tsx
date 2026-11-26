import { useState } from 'react';
import { useTranslation } from '../../../locale/react';
import { Menu } from '../../../presentation/primitives/Menu';
import { workbenchFilesCss } from '../../../presentation/right-panel/workbenchFilesCss';
import type { Viewer } from './definitions';

export function ViewerMenu({ choices, mode, select }: { choices: Viewer[]; mode?: Viewer; select: (mode: Viewer) => void }) {
  const tx = useTranslation(), [open, setOpen] = useState(false);
  if (!mode || choices.length < 2) return null;
  return <Menu open={open} onClose={() => setOpen(false)} align="end" dense autoFocus selectedId={mode}
    items={choices.map(id => ({ id, label: tx(`artifacts:workbench.${id}`) }))}
    onSelect={id => { select(id as Viewer); setOpen(false); }}
    anchor={<button type="button" className={`${workbenchFilesCss.tool} ${workbenchFilesCss.viewer}`} aria-label={tx('artifacts:workbench.viewer')} aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen(!open)}>{tx(`artifacts:workbench.${mode}`)}</button>}/>;
}
