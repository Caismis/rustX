import { createRoot } from 'react-dom/client';
import { AppServerClient } from './client/app-server';
import { App } from './app/App';
import './presentation/theme/base.css';
import './presentation/theme/tokens.css';
import './app/console.css';
// One connection per page, outside React component lifecycle.
const client = new AppServerClient();
createRoot(document.getElementById('root')!).render(<App client={client} />);
