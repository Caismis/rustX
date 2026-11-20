import { useContext, useMemo, useRef } from 'react';
import { ExpressParser } from '@a2ui/agent';
import { Catalog } from '@a2ui/web_core/catalog';
import { createGenerativeUIDispatcher, GenerativeUISurfaceManager, GenerativeUISurfaceContentSchema } from '@copilotkit/core/generative-ui';
import { GenerativeUIRuntimeContext, SurfaceView, type GenerativeUIRuntime } from '@copilotkit/react-core/v2/generative-ui/native';
import { ExpressCard, ExpressEventContext } from './ExpressCard';
import type { ExpressSurface } from './a2ui-express';
import { MAX_SOURCE_LENGTH } from './express-values';
import { MarkdownText } from './MarkdownText';
import catalog from './vendor/playground/catalog.json';
import { playgroundUI } from './vendor/playground/ui';
import { shadcnTheme } from './vendor/playground/design-systems/theme';
import css from './ExpressCard.module.css';

const inlineTarget = 'chat';
const openLink = '__rustx_open_https_link';
const parser = new ExpressParser(Catalog.fromJson(catalog, '1.0', catalog.catalogId), 'inline', '1.0');

function https(value: unknown): string | undefined {
  if (typeof value !== 'string' || !/^https:\/\//i.test(value.trim())) return undefined;
  try { return new URL(value).href; } catch { return undefined; }
}

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
    const operations = parser.compile(body).filter(operation => !('callRendererFunction' in operation));
    const created = operations.find(operation => 'createSurface' in operation);
    if (!created?.createSurface) return undefined;
    const { surfaceId } = created.createSurface;
    const content = GenerativeUISurfaceContentSchema.parse({
      surfaceId, catalogId: catalog.catalogId, target: inlineTarget, operations: guard(operations), status: 'ready', rejected: [],
    });
    const manager = new GenerativeUISurfaceManager(playgroundUI);
    manager.ingest('inline', undefined, content);
    const surface = manager.getSurface('inline', surfaceId);
    if (!surface) return undefined;
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
          const url = https(context.url);
          if (url) window.open(url, '_blank', 'noopener,noreferrer');
        } else if (send) {
          const text = userMessage?.trim() || (Object.keys(context).length ? name + '\n' + JSON.stringify(context) : name);
          if (text.length <= 16384) send(text);
        }
      }
      return { ok: true, result: undefined };
    },
    reportError: () => {},
  }), [send, streaming, view]);

  if (!view || !runtime) return tree ? <ExpressCard surface={tree} streaming={streaming}/> : <MarkdownText text={fallback}/>;
  return <div className={`${css.surface} rustx-playground`} data-a2ui-express="" data-a2ui-catalog="playground" data-ds="shadcn"
    onClickCapture={event => {
      const input = event.nativeEvent;
      if (input.isTrusted) {
        activation.current = input;
        setTimeout(() => { if (activation.current === input) activation.current = undefined; }, 0);
      }
    }}>
    <GenerativeUIRuntimeContext value={runtime}><SurfaceView surface={view.surface}/></GenerativeUIRuntimeContext>
  </div>;
}
