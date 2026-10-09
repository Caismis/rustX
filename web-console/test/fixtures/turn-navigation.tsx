import { createRoot } from 'react-dom/client';
import { useRef, useState } from 'react';
import { ChatViewport } from '../../src/presentation/layout/ChatViewport';
import '../../src/presentation/theme/base.css';
import '../../src/app/console.css';
function Fixture() {
  const viewport = useRef<ChatViewport>(null);
  const [active, setActive] = useState<string | null>();
  return <div style={{ height: 480, display: 'flex', flexDirection: 'column' }}>
    <nav>{['first', 'last'].map(id => <button key={id} aria-current={active === `turn:${id}` ? 'true' : undefined} onClick={() => viewport.current?.beginNavigation().commit(`turn:${id}`)}>{id}</button>)}</nav>
    <ChatViewport ref={viewport} onActiveTurn={setActive}>
      <div data-chat-anchor-key="turn:first" data-chat-turn-owner="turn:first" style={{ height: 1100 }}>Long first turn</div>
      <div data-chat-anchor-key="turn:last" style={{ height: 110 }}>Final prompt</div>
      <div data-chat-turn-owner="turn:last" style={{ height: 60 }}>Short final reply</div>
    </ChatViewport>
  </div>;
}
createRoot(document.getElementById('root')!).render(<Fixture/>);
