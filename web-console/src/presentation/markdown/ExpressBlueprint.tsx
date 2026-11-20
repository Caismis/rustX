import { useDeferredValue, useEffect, useRef, useState } from 'react';
import { MAX_SOURCE_LENGTH } from './express-values';
import { playgroundParser } from './playground-parser';
import css from './ExpressGeneration.module.css';

type Part = { id: string; component?: string; height?: number; children: Part[] };
type Component = { id: string; component: string; [key: string]: unknown };
const layouts = new Set(['Column', 'Row', 'Grid', 'Card', 'Section', 'Box', 'List']);
const text = new Set(['Text', 'Title', 'Heading', 'Caption', 'Label', 'Divider']);
const typicalHeight: Record<string, number> = {
  StockTicker: 218, WeatherCard: 344, DataTable: 218, KeyValueList: 150,
  Image: 180, Svg: 160, HtmlView: 160, Stat: 94, Alert: 64,
};

function childrenOf(node: Component): string[] {
  const value = node.children ?? node.child;
  if (typeof value === 'string') return [value];
  if (Array.isArray(value)) return value.filter(id => typeof id === 'string');
  return [];
}

function blueprint(components: Map<string, Component>): Part | undefined {
  if (!components.has('root')) return undefined;
  let remaining = 32;
  const parents = new Set<string>();
  function visit(id: string, depth: number): Part | undefined {
    if (depth > 12 || remaining <= 0 || parents.has(id)) return undefined;
    remaining--;
    const node = components.get(id);
    const children: Part[] = [];
    if (node && layouts.has(node.component)) {
      parents.add(id);
      for (const child of childrenOf(node)) {
        const part = visit(child, depth + 1);
        if (part) children.push(part);
        if (!remaining) break;
      }
      parents.delete(id);
    }
    return {
      id,
      component: node?.component,
      height: typeof node?.height === 'number' && Number.isFinite(node.height) ? node.height : undefined,
      children,
    };
  }
  return visit('root', 0);
}

function Frame({ part }: { part: Part }) {
  if (part.children.length) {
    const horizontal = part.component === 'Row' || part.component === 'Grid';
    return <div className={horizontal ? css.row : css.stack}>
      {part.children.map((child, index) => <Frame key={child.id + ':' + index} part={child}/>)}
    </div>;
  }
  const name = part.component;
  const height = part.height ?? (name && Object.hasOwn(typicalHeight, name) ? typicalHeight[name] :
    name && /Chart$|^(?:Heatmap|Treemap|Sankey|MapView|RouteMap)$/.test(name) ? 242 :
      name && text.has(name) ? 34 : 80);
  return <div className={css.frame} style={{ minBlockSize: Math.min(360, Math.max(28, height)) }}
    data-a2ui-blueprint={name ?? ''}>
    {name && <span className={css.caption}>{name}</span>}
  </div>;
}

function openDraft() {
  return {
    stream: playgroundParser.openStream({ wrapped: false, afterRejectedBlock: 'continue' }),
    components: new Map<string, Component>(),
    source: '',
  };
}

export default function ExpressBlueprint({ source }: { source: string }) {
  const deferred = useDeferredValue(source);
  const reader = useRef<ReturnType<typeof openDraft> | undefined>(undefined);
  const [tree, setTree] = useState<Part>();
  useEffect(() => {
    if (deferred.length > MAX_SOURCE_LENGTH) {
      reader.current = undefined;
      setTree(undefined);
      return;
    }
    let draft = reader.current;
    if (!draft || !deferred.startsWith(draft.source)) {
      draft = openDraft();
      reader.current = draft;
      setTree(undefined);
    }
    const delta = deferred.slice(draft.source.length);
    if (!delta) return;
    draft.source = deferred;
    // Render geometry only. Draft props never reach a renderer or event handler.
    let changed = false;
    try {
      for (const part of draft.stream.feed(delta)) {
        if (part.type !== 'a2ui') continue;
        for (const op of part.a2ui) {
          const components = 'createSurface' in op ? op.createSurface?.components :
            'updateComponents' in op ? op.updateComponents?.components : undefined;
          for (const component of components ?? []) {
            if (typeof component.id !== 'string' || typeof component.component !== 'string') continue;
            draft.components.set(component.id, component as Component);
            changed = true;
          }
        }
      }
    } catch {
      // A rejected stream can still finish through the settled chat renderer.
    }
    if (changed) setTree(blueprint(draft.components));
  }, [deferred]);

  return tree ? <Frame part={tree}/> : <div className={css.emptyFrame}/>;
}
