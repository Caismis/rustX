import { useContext, useMemo, useRef } from 'react';
import { createGenerativeUIDispatcher, GenerativeUISurfaceManager, GenerativeUISurfaceContentSchema } from '@copilotkit/core/generative-ui';
import { GenerativeUIRuntimeContext, SurfaceView, type GenerativeUIRuntime } from '@copilotkit/react-core/v2/generative-ui/native';
import { ExpressCard, ExpressEventContext } from './ExpressCard';
import type { ExpressSurface } from './a2ui-express';
import { MAX_SOURCE_LENGTH } from './express-values';
import { MarkdownText } from './MarkdownText';
import { playgroundParser } from './playground-parser';
import catalog from './vendor/playground/catalog.json';
import { playgroundUI } from './vendor/playground/ui';
import { shadcnTheme } from './vendor/playground/design-systems/theme';
import { httpsUrl } from './vendor/playground/a2ui/catalog/renderers/format';
import css from './ExpressCard.module.css';

const inlineTarget = 'chat';
const openLink = '__rustx_open_https_link';

function guard(value: unknown, depth = 0): unknown {
  if (depth > 96) throw new Error('Playground surface nesting limit exceeded');
  if (Array.isArray(value)) return value.map(item => guard(item, depth + 1));
  if (value === null || typeof value !== 'object') return value;
  const record = value as Record<string, unknown>;
  const action = record.functionCall;
  if (action && typeof action === 'object' && '@call' in action && action['@call'] === 'openUrl') {
    const args = 'args' in action && action.args && typeof action.args === 'object' ? action.args as Record<string, unknown> : {};
    return { event: { name: openLink, context: { url: args.url } } };
  }
  return Object.fromEntries(Object.entries(record)
    .filter(([key]) => key !== 'onVisible' || typeof record.component !== 'string')
    .map(([key, item]) => [key, guard(item, depth + 1)]));
}

export function createSurface(source: string) {
  if (source.length > MAX_SOURCE_LENGTH) return undefined;
  try {
    const body = source.trim().replace(/^<a2ui>\s*/, '').replace(/\s*<\/a2ui>$/, '');
    const operations = playgroundParser.compile(body).filter(operation => !('callRendererFunction' in operation));
    const createdOps = operations.filter(operation => 'createSurface' in operation);
    if (createdOps.length !== 1 || !createdOps[0]?.createSurface) return undefined;
    const { surfaceId } = createdOps[0].createSurface;
    const content = GenerativeUISurfaceContentSchema.parse({
      surfaceId, catalogId: catalog.catalogId, target: inlineTarget, operations: guard(operations), status: 'ready', rejected: [],
    });
    const manager = new GenerativeUISurfaceManager(playgroundUI);
    manager.ingest('inline', undefined, content);
    const surface = manager.getSurface('inline', surfaceId);
    if (!surface || surface.failed || surface.clientRejected.length > 0 || !surface.model.componentsModel.get('root')) {
      return undefined;
    }
    return { manager, surface };
  } catch {
    return undefined;
  }
}

export default function PlaygroundExpress({ source, fallback, tree, streaming }: {
  source: string; fallback: string; tree?: ExpressSurface; streaming: boolean;
}) {
  const send = useContext(ExpressEventContext);
  const activation = useRef<Event | undefined>(undefined);
  const view = useMemo(() => createSurface(source), [source]);
  const runtime = useMemo<GenerativeUIRuntime | undefined>(() => view && ({
    manager: view.manager,
    dispatcher: createGenerativeUIDispatcher({ getDeclared: () => new Map() }),
    ui: playgroundUI,
    config: { ui: playgroundUI },
    transport: 'tool',
    theme: shadcnTheme,
    forwardEvent: async ({ name, userMessage, context }) => {
      if (activation.current?.isTrusted && !streaming) {
        if (name === openLink) {
          const url = httpsUrl(context && typeof context === 'object' ? context.url : undefined);
          if (url) window.open(url, '_blank', 'noopener,noreferrer');
        } else if (send) {
          const trimmedName = typeof name === 'string' ? name.trim() : '';
          const message = typeof userMessage === 'string' ? userMessage.trim() : '';
          const payload = context && typeof context === 'object' && !Array.isArray(context) && Object.keys(context).length > 0
            ? context : undefined;
          const text = message || (trimmedName ? (payload ? trimmedName + '\n' + JSON.stringify(payload) : trimmedName) : '');
          if (text && text.length <= 16384) send(text);
        }
      }
      return { ok: true, result: undefined };
    },
    reportError: () => {},
  }), [send, streaming, view]);

  if (!view || !runtime) return tree ? <ExpressCard surface={tree} streaming={streaming}/> : <MarkdownText text={fallback}/>;
  function activate(nativeEvent: Event) {
    if (!nativeEvent.isTrusted) return;
    activation.current = nativeEvent;
    setTimeout(() => { if (activation.current === nativeEvent) activation.current = undefined; }, 0);
  }
  return <div className={`${css.surface} rustx-playground`} data-a2ui-express="" data-a2ui-catalog="playground" data-ds="shadcn"
    onClickCapture={({ nativeEvent }) => activate(nativeEvent)}
    onKeyDownCapture={({ nativeEvent, key, repeat, target }) => {
      if (!repeat && (key === 'Enter' || key === ' ') && target instanceof Element &&
        target.matches('div[data-a2ui-input][role="button"], div[data-a2ui-entity-card][role="link"]')) activate(nativeEvent);
    }}>
    <GenerativeUIRuntimeContext value={runtime}><SurfaceView surface={view.surface}/></GenerativeUIRuntimeContext>
  </div>;
}
