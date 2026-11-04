import { useLayoutEffect, useRef, useState, type CSSProperties } from 'react';

/** Expand an editor inside the inspector while retaining its DOM and scroll slot. */
export function useInspectorExpansion(control?: { expanded: boolean; onChange: (expanded: boolean) => void }) {
  const anchor = useRef<HTMLDivElement>(null),
    [localExpanded, setLocalExpanded] = useState(false),
    [collapsedHeight, setCollapsedHeight] = useState(0),
    [bounds, setBounds] = useState<CSSProperties>();
  const expanded = control?.expanded ?? localExpanded;
  const setExpanded = (value: boolean) => {
    if (control) control.onChange(value);
    else setLocalExpanded(value);
  };
  useLayoutEffect(() => {
    if (!expanded) return;
    const body = anchor.current?.closest<HTMLElement>('[data-workflow-inspector-body]');
    if (!body) return;
    const measure = () => {
      const box = body.getBoundingClientRect(),
        viewport = window.visualViewport,
        bottom = viewport ? viewport.offsetTop + viewport.height : window.innerHeight;
      setBounds({
        top: box.top,
        left: box.left,
        width: box.width,
        height: Math.max(64, Math.min(box.bottom, bottom) - box.top),
      });
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(body);
    window.addEventListener('resize', measure);
    window.visualViewport?.addEventListener('resize', measure);
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', measure);
      window.visualViewport?.removeEventListener('resize', measure);
    };
  }, [expanded]);
  const toggle = () => {
    if (!expanded) setCollapsedHeight(anchor.current?.getBoundingClientRect().height ?? 0);
    setExpanded(!expanded);
  };
  return { anchor, expanded, setExpanded, toggle, collapsedHeight, bounds };
}
