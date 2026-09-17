// jsdom has no layout engine. Geometry is exercised in real Chromium.
import { beforeEach, vi } from 'vitest';
beforeEach(() => { vi.stubGlobal('ResizeObserver', class { observe() {} unobserve() {} disconnect() {} }); });
