import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { AppFrame } from '../../src/presentation/layout/AppFrame';
import { Button } from '../../src/presentation/primitives/Button';
import { Input } from '../../src/presentation/primitives/Input';
import { Menu } from '../../src/presentation/primitives/Menu';
import { HoverCard } from '../../src/presentation/primitives/HoverCard';
import { Modal } from '../../src/presentation/primitives/Modal';
import { DisclosureRow } from '../../src/presentation/primitives/DisclosureRow';
import { MarkdownText } from '../../src/presentation/markdown/MarkdownText';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/gradient-shadow-text.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/shiki.css';
import '../../src/presentation/theme/reset.css';

/** A long, non-wrapping dynamic label, a submenu too tall for a short
 * viewport and a short one with a disabled row: the shared Menu geometry
 * cases. */
const nestedItems = [
  { id: 'long', label: 'A deliberately long dynamic label that must truncate at the design width of the shared menu card' },
  { id: 'more', label: 'More options', submenu: Array.from({ length: 40 }, (_, index) => ({ id: `option-${index + 1}`, label: `Option ${index + 1}` })) },
  { id: 'sort', label: 'Sort by', submenu: [{ id: 'name', label: 'Name' }, { id: 'size', label: 'Size', disabled: true }, { id: 'date', label: 'Date' }] },
  ...['Tango', 'Uniform', 'Victor', 'Whiskey', 'X-ray', 'Yankee'].map(label => ({ id: label.toLowerCase(), label })),
];

function Fixture() {
  const [menu, setMenu] = useState(false);
  const [nested, setNested] = useState(false);
  const [nestedSelected, setNestedSelected] = useState('None');
  const [modal, setModal] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [selected, setSelected] = useState('None');
  const [submitted, setSubmitted] = useState(0);
  return <AppFrame sidebar={() => <div>Navigation fixture</div>}>
    <h1>Foundation contracts</h1>
    <form onSubmit={event => { event.preventDefault(); setSubmitted(value => value + 1); }}>
      <Input aria-label="Form value" /><Button>Ordinary button</Button><Button type="submit">Submit form</Button><Button disabled>Disabled button</Button>
      <output aria-label="Submissions">{submitted}</output>
    </form>
    <Menu open={menu} onClose={() => setMenu(false)} autoFocus anchor={<Button onClick={() => setMenu(!menu)}>Actions</Button>} items={[{ id: 'a', label: 'Alpha' }, { id: 'b', label: 'Blocked', disabled: true }, { id: 'c', label: 'Charlie' }]} onSelect={id => { setSelected(id); setMenu(false); }} />
    <output aria-label="Selection">{selected}</output>
    <Menu open={nested} onClose={() => setNested(false)} autoFocus anchor={<Button onClick={() => setNested(!nested)}>Nested actions</Button>} items={nestedItems} onSelect={id => { setNestedSelected(id); setNested(false); }} />
    <output aria-label="Nested choice">{nestedSelected}</output>
    <HoverCard anchor={<Button>Details</Button>} content={<p>Hover details</p>} copyLabel="Copy" copiedLabel="Copied" />
    <Button onClick={() => setModal(true)}>Open dialog</Button>
    <Modal closeLabel="Close dialog" open={modal} onClose={() => setModal(false)} title="Example dialog" footer={<Button onClick={() => setModal(false)}>Done</Button>}><Input aria-label="Dialog value" /></Modal>
    <DisclosureRow icon={null} title="Expand details" expandable open={expanded} expandOnRowClick onToggle={() => setExpanded(value => !value)}><p>Expanded content</p></DisclosureRow>
    <Button>Outside target</Button>
    <MarkdownText text={'# Rich content\n\n**Strong** and `inline` with [a link](https://example.com).\n\n| First | Second |\n| - | - |\n| Value | Other |\n\n```rust\nfn main() { println!("hello"); }\n```\n\nMath $E=mc^2$.'} />
  </AppFrame>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
