import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { CardMenu } from '../../src/presentation/catalog/CardMenu';
import '../../src/components/ui/coss.css';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/gradient-shadow-text.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/shiki.css';
import '../../src/presentation/theme/reset.css';

function Harness() {
  const [disabled, setDisabled] = useState(false);
  const [token, setToken] = useState('opened');
  const [selected, setSelected] = useState('');
  window.menuGesture = { setDisabled, setToken };
  return <>
    <CardMenu label="Actions for guide" items={[
      { id: 'edit', label: 'Edit', submenu: [{ id: 'rename', label: 'Rename' }] },
      { id: 'move', label: 'Move', disabled },
    ]} onSelect={id => setSelected(`${token}:${id}`)} />
    <output data-testid="selected">{selected}</output>
  </>;
}

declare global { interface Window { menuGesture: { setDisabled: (disabled: boolean) => void; setToken: (token: string) => void } } }

createRoot(document.getElementById('root')!).render(<Harness />);
