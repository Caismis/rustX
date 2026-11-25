import type { ComponentType } from 'react';
import { createGenerativeUI, standardUI } from '@copilotkit/react-core/v2/generative-ui';
import { ownEntries } from './a2ui/catalog/entries';
import { CATALOG_ID } from './a2ui/catalog/ids';
import { chartRenderers } from './a2ui/catalog/renderers/charts';
import { contentRenderers } from './a2ui/catalog/renderers/content';
import { dataRenderers } from './a2ui/catalog/renderers/data';
import { domainRenderers } from './a2ui/catalog/renderers/domain';
import { feedbackRenderers } from './a2ui/catalog/renderers/feedback';
import { inputRenderers } from './a2ui/catalog/renderers/inputs';
import { layoutRenderers } from './a2ui/catalog/renderers/layout';
import { mapRenderers } from './a2ui/catalog/renderers/maps';
import { httpsUrl } from './a2ui/catalog/renderers/format';
import type { ComponentName } from './a2ui/catalog/definitions';
import { cossOwn, cossStandard } from './design-systems/coss';
import type { StandardProps } from './design-systems/shared';
import './surface.css';

const renderers = {
  ...chartRenderers, ...contentRenderers, ...dataRenderers, ...domainRenderers,
  ...feedbackRenderers, ...inputRenderers, ...layoutRenderers, ...mapRenderers, ...cossOwn,
};

type Renderer = ComponentType<{ props: Record<string, unknown> }>;
function own(name: ComponentName) {
  const Render = renderers[name] as unknown as Renderer;
  function Node({ onAction, ...props }: Record<string, unknown>) {
    const action = typeof onAction === 'function' ? () => { void onAction({}); } : undefined;
    return <div data-a2-type={name} style={{ display: 'contents' }}><Render props={{ ...props, action }}/></div>;
  }
  return Node;
}

function standard(name: string, Base: ComponentType<StandardProps>) {
  const Render = cossStandard[name] ?? Base;
  const csp = `<!doctype html><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; form-action 'none'; base-uri 'none'">`;
  return (props: StandardProps) => {
    // The standard HtmlView owns an opaque, scriptless sandbox. Restrict
    // its subresources as well, while leaving inline HTML and SVG available.
    const safe = name === 'HtmlView' && typeof props.html === 'string'
      ? { ...props, html: csp + props.html }
      : 'href' in props ? { ...props, href: httpsUrl(props.href) } : props;
    const actions = name === 'EntityCard' && Array.isArray(props.actions)
      ? { actions: props.actions.map(action => action && typeof action === 'object'
          ? { ...action, href: httpsUrl(action.href) } : action) }
      : {};
    return <span data-a2-type={name} style={{ display: 'contents' }}><Render {...safe} {...actions}/></span>;
  };
}

export const playgroundUI = createGenerativeUI({
  id: CATALOG_ID,
  extends: [standardUI],
  overrides: Object.fromEntries([...standardUI.components].map(([name, entry]) => [
    name, { component: standard(name, entry.component as ComponentType<StandardProps>) },
  ])) as never,
  components: ownEntries(own) as never,
});
