import { useCallback, useLayoutEffect, useMemo, useRef, useState, type DOMAttributes, type KeyboardEvent, type RefObject } from 'react';

export interface ViewportMetrics {
  readonly top: number;
  readonly floor: number;
  readonly height: number;
}

export function scrollMetrics(element: HTMLElement): ViewportMetrics {
  const height = element.clientHeight;
  return { top: element.scrollTop, height, floor: Math.max(0, element.scrollHeight - height) };
}

export class ScrollFollow {
  private static readonly owners = new WeakMap<HTMLElement, ScrollFollow>();
  private target: number | null = null;
  private sampledTop: number | undefined;

  constructor(private following: boolean, private readonly threshold: number) {}

  static forElement(element: HTMLElement): ScrollFollow | undefined { return this.owners.get(element); }

  bind(element: HTMLElement): () => void {
    ScrollFollow.owners.set(element, this);
    return () => { if (ScrollFollow.owners.get(element) === this) ScrollFollow.owners.delete(element); };
  }

  get active(): boolean { return this.following; }
  get animating(): boolean { return this.target !== null; }

  nearBottom(metrics: ViewportMetrics): boolean {
    return Math.abs(metrics.floor - metrics.top) <= this.threshold;
  }

  setFollowing(active: boolean): void {
    this.following = active;
    if (!active) this.target = null;
  }

  reset(): void {
    this.setFollowing(false);
    this.sampledTop = undefined;
  }

  sample(
    metrics: ViewportMetrics,
    movedByReader = this.sampledTop === undefined || Math.abs(metrics.top - this.sampledTop) > 0.5,
  ): boolean {
    this.sampledTop = metrics.top;
    if (!this.animating && movedByReader) this.following = this.nearBottom(metrics);
    return this.active;
  }

  settle(metrics: ViewportMetrics): boolean {
    const target = this.target;
    this.target = null;
    return this.sample(
      metrics,
      target === null ? undefined : Math.abs(metrics.top - Math.min(target, metrics.floor)) > this.threshold,
    );
  }

  jump(element: HTMLElement, metrics: ViewportMetrics, top: number): ViewportMetrics {
    const animated = this.animating;
    this.target = null;
    const target = Math.max(0, Math.min(metrics.floor, top));
    if (animated && typeof element.scrollTo === 'function') element.scrollTo({ top: target, behavior: 'instant' });
    else if (target !== metrics.top) element.scrollTop = target;
    const landed = { ...metrics, top: element.scrollTop };
    this.sampledTop = landed.top;
    this.following = this.nearBottom(landed);
    return landed;
  }

  toBottom(element: HTMLElement, metrics: ViewportMetrics, behavior: 'instant' | 'smooth'): ViewportMetrics {
    this.following = true;
    if (metrics.top === metrics.floor) return this.jump(element, metrics, metrics.floor);
    if (behavior === 'instant' && this.animating) {
      if (this.target !== metrics.floor && typeof element.scrollTo === 'function') {
        this.target = metrics.floor;
        element.scrollTo({ top: metrics.floor, behavior: 'smooth' });
      }
      return metrics;
    }
    if (behavior === 'instant' || (!this.animating && this.nearBottom(metrics))) {
      return this.jump(element, metrics, metrics.floor);
    }
    if (typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches) {
      return this.jump(element, metrics, metrics.floor);
    }
    if (this.target === null && typeof element.scrollTo === 'function') {
      this.target = metrics.floor;
      element.scrollTo({ top: metrics.floor, behavior: 'smooth' });
    } else if (typeof element.scrollTo !== 'function') {
      return this.jump(element, metrics, metrics.floor);
    }
    return metrics;
  }

  interrupt(element: HTMLElement, metrics: ViewportMetrics): void {
    if (!this.animating) return;
    this.target = null;
    this.sampledTop = metrics.top;
    if (typeof element.scrollTo === 'function') element.scrollTo({ top: metrics.top, behavior: 'instant' });
  }
}

export function useScrollFollow(initial: boolean, threshold: number): ScrollFollow {
  const [follow] = useState(() => new ScrollFollow(initial, threshold));
  return follow;
}

interface ScrollEdges { readonly canScrollUp: boolean; readonly canScrollDown: boolean }
const AT_REST: ScrollEdges = { canScrollUp: false, canScrollDown: false };
const SCROLL_KEYS = new Set(['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', ' ']);

export function useProcessScroll(
  bodyRef: RefObject<HTMLDivElement | null>,
  contentRef: RefObject<HTMLDivElement | null>,
  open: boolean,
  grouped: boolean,
): {
  edges: ScrollEdges;
  events: Pick<DOMAttributes<HTMLDivElement>, 'onScroll' | 'onWheel' | 'onTouchStart' | 'onPointerDown' | 'onKeyDown'>;
  initialize: (position: 'top' | 'bottom') => void;
} {
  const follow = useScrollFollow(false, 1);
  const initialPosition = useRef<'top' | 'bottom' | null>(null);
  const [edges, setEdges] = useState<ScrollEdges>(AT_REST);

  const initialize = useCallback((position: 'top' | 'bottom') => {
    initialPosition.current = position;
  }, []);

  const sync = useCallback((cause: 'resize' | 'scroll' | 'scrollend') => {
    const body = bodyRef.current;
    let next = AT_REST;
    if (body !== null && body.closest('[hidden], [data-group-expanded-mode]') === null) {
      let metrics = scrollMetrics(body);
      const initial = cause === 'resize' ? initialPosition.current : null;
      if (initial !== null) {
        metrics = follow.jump(body, metrics, initial === 'bottom' ? metrics.floor : 0);
        if (initial === 'top') follow.setFollowing(false);
        initialPosition.current = null;
      } else {
        const wasAnimating = follow.animating;
        if (cause === 'scrollend') follow.settle(metrics);
        else follow.sample(metrics);
        if (follow.active && (cause === 'resize' || (cause === 'scrollend' && wasAnimating))) {
          metrics = follow.toBottom(body, metrics, 'smooth');
        }
      }
      next = { canScrollUp: metrics.top > 1, canScrollDown: metrics.top < metrics.floor - 1 };
    } else {
      follow.reset();
    }
    setEdges(previous => previous.canScrollUp === next.canScrollUp && previous.canScrollDown === next.canScrollDown
      ? previous : next);
  }, [bodyRef, follow]);

  const interrupt = useCallback(() => {
    const body = bodyRef.current;
    if (body !== null && follow.animating) follow.interrupt(body, scrollMetrics(body));
  }, [bodyRef, follow]);

  const events = useMemo(() => ({
    onScroll: () => { sync('scroll'); },
    onWheel: interrupt,
    onTouchStart: interrupt,
    onPointerDown: interrupt,
    onKeyDown: (event: KeyboardEvent<HTMLDivElement>) => {
      if (!event.defaultPrevented && SCROLL_KEYS.has(event.key)) interrupt();
    },
  }), [interrupt, sync]);

  useLayoutEffect(() => {
    interrupt();
    follow.reset();
    if (!grouped || !open) initialPosition.current = null;
  }, [follow, grouped, interrupt, open]);

  useLayoutEffect(() => {
    const body = bodyRef.current;
    if (body === null || !open) return;
    const unbind = follow.bind(body);
    sync('resize');
    const onScrollEnd = (event: Event): void => { if (event.target === body) sync('scrollend'); };
    body.addEventListener('scrollend', onScrollEnd);
    if (typeof ResizeObserver === 'undefined') {
      return () => {
        unbind();
        body.removeEventListener('scrollend', onScrollEnd);
      };
    }
    const observer = new ResizeObserver(() => { sync('resize'); });
    observer.observe(body);
    if (contentRef.current !== null) observer.observe(contentRef.current);
    return () => {
      unbind();
      observer.disconnect();
      body.removeEventListener('scrollend', onScrollEnd);
    };
  }, [bodyRef, contentRef, follow, open, sync]);

  return { edges, events, initialize };
}
