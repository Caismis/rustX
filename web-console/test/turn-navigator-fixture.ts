/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-chat tests/turn-navigator-fixture.ts.
import { vi } from 'vitest';
import { act } from '@testing-library/react';

/** Supply rail viewport sizes and element-local scrolling absent from jsdom.
 * Only the rail scroller (the direct child of its `nav`) is observed.
 * @param initialHeight - first delivered height, or null to delay initial layout. */
export function installTurnNavigatorObserver(initialHeight: number | null = 300) {
  const rails = new Map<Element, Observer>();
  let height = initialHeight;
  class Observer implements ResizeObserver {
    constructor(private readonly callback: ResizeObserverCallback) {}
    observe(element: Element): void {
      if (!(element instanceof HTMLElement) || element.parentElement?.tagName !== 'NAV') return;
      rails.set(element, this);
      element.scrollTo = (options: ScrollToOptions | number = {}, y?: number) => {
        const next = typeof options === 'number' ? y ?? 0 : options.top ?? element.scrollTop;
        if (next === element.scrollTop) return;
        element.scrollTop = next;
        queueMicrotask(() => { if (element.isConnected) act(() => { element.dispatchEvent(new Event('scroll')); }); });
      };
      queueMicrotask(() => {
        const current = height;
        if (current !== null && rails.get(element) === this) act(() => { this.deliver(element, current); });
      });
    }
    unobserve(element: Element): void { rails.delete(element); }
    disconnect(): void { for (const [element, observer] of rails) if (observer === this) rails.delete(element); }
    deliver(target: Element, blockSize: number): void {
      const size = [{ inlineSize: 28, blockSize }];
      this.callback([{ target, borderBoxSize: size, contentBoxSize: size, devicePixelContentBoxSize: size, contentRect: new DOMRectReadOnly(0, 0, 28, blockSize) }], this);
    }
  }
  vi.stubGlobal('ResizeObserver', Observer);
  return { resize(next: number) { height = next; for (const [element, observer] of rails) observer.deliver(element, next); } };
}

/** A ResizeObserver stub for whole-conversation tests: the rail scroller
 * receives a fixed height, its marks need none, and every observer outside
 * the rail is handed to `other`. */
export function railAwareResizeObserver(other: (resize: () => void) => void, railHeight = 300) {
  return class {
    constructor(private readonly callback: ResizeObserverCallback) {}
    observe(element: Element): void {
      if (element instanceof HTMLElement && element.parentElement?.tagName === 'NAV') {
        const size = [{ inlineSize: 28, blockSize: railHeight }];
        queueMicrotask(() => act(() => {
          this.callback([{ target: element, borderBoxSize: size, contentBoxSize: size, devicePixelContentBoxSize: size, contentRect: new DOMRectReadOnly(0, 0, 28, railHeight) }], this as unknown as ResizeObserver);
        }));
      } else if (!element.closest('nav')) other(() => this.callback([], this as unknown as ResizeObserver));
      // Fixed-pitch marks need no measurement.
    }
    unobserve(): void {}
    disconnect(): void {}
  };
}
