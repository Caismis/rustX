import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { AppFrame } from '../../src/presentation/layout/AppFrame';
import { Button } from '../../src/presentation/primitives/Button';
import { Input } from '../../src/presentation/primitives/Input';
import { Menu } from '../../src/presentation/primitives/Menu';
import { Popover } from '../../src/presentation/primitives/Popover';
import { Dialog } from '../../src/presentation/primitives/Dialog';
import { DisclosureRow } from '../../src/presentation/primitives/DisclosureRow';
import { MarkdownText } from '../../src/presentation/markdown/MarkdownText';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/tokens.css';
import '../../src/presentation/theme/shiki.css';

function Fixture() {
  const [modal, setModal] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [selected, setSelected] = useState('None');
  const [submitted, setSubmitted] = useState(0);
  return <AppFrame navigation={<div>Navigation fixture</div>} dock={<div>Dock fixture</div>}>
    <h1>Foundation contracts</h1>
    <form onSubmit={event => { event.preventDefault(); setSubmitted(value => value + 1); }}>
      <Input aria-label="Form value" /><Button>Ordinary button</Button><Button type="submit">Submit form</Button><Button disabled>Disabled button</Button>
      <output aria-label="Submissions">{submitted}</output>
    </form>
    <Menu label="Actions" items={[{ id: 'a', label: 'Alpha' }, { id: 'b', label: 'Blocked', disabled: true }, { id: 'c', label: 'Charlie' }]} onSelect={setSelected} />
    <output aria-label="Selection">{selected}</output>
    <Popover label="Details"><Input aria-label="Popover value" /><Button>Popover action</Button></Popover>
    <Button onClick={() => setModal(true)}>Open dialog</Button>
    <Dialog open={modal} onClose={() => setModal(false)} title="Example dialog" footer={<Button onClick={() => setModal(false)}>Done</Button>}><Input aria-label="Dialog value" /><Popover label="Nested details"><Button>Nested action</Button></Popover></Dialog>
    <DisclosureRow icon={null} title="Expand details" expandable open={expanded} expandOnRowClick onToggle={() => setExpanded(value => !value)}><p>Expanded content</p></DisclosureRow>
    <Button>Outside target</Button>
    <MarkdownText text={'# Rich content\n\n**Strong** and `inline` with [a link](https://example.com).\n\n| First | Second |\n| - | - |\n| Value | Other |\n\n```rust\nfn main() { println!("hello"); }\n```\n\nMath $E=mc^2$.'} />
  </AppFrame>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
