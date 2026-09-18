import { createRoot } from 'react-dom/client';
import { AppServerClient } from './client/app-server';
import { App } from './app/App';
import { ConnectionController } from './connection/controller';
import './presentation/theme/base.css';
import './presentation/theme/gradient-shadow-text.css';
import './presentation/theme/design-platform.css';
import './presentation/theme/shiki.css';
import './presentation/theme/corner-shape.css';
import './presentation/theme/scrollbar.css';
import './presentation/theme/reset.css';
import './app/console.css';
// One connection per page, outside React component lifecycle.
const client = new AppServerClient();
const connection = new ConnectionController(client);
createRoot(document.getElementById('root')!).render(<App client={client} connection={connection} />);
void connection.start();
