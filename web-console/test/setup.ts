// jsdom has no layout engine. Geometry is exercised in real Chromium.
import { beforeEach, vi } from 'vitest';
beforeEach(() => { vi.stubGlobal('ResizeObserver', class { observe() {} unobserve() {} disconnect() {} }); });
// Without layout every element measures as an empty rect at the origin, which
// Floating UI's `hide` reads as a reference out of layout: every Menu would
// close as it opens. Whether an anchor is rendered is a layout fact, proven in
// Chromium; here every anchor counts as rendered.
vi.mock('@floating-ui/react-dom', async importOriginal => ({
  ...await importOriginal<typeof import('@floating-ui/react-dom')>(),
  hide: () => ({ name: 'hide', fn: () => ({}) }),
}));
// Whether a focused control is still rendered is the same layout fact (see
// `SettingsPanel`'s navigation handoff), and jsdom has no `checkVisibility`;
// node-environment suites have no DOM at all.
if (typeof Element !== 'undefined') Element.prototype.checkVisibility = () => true;
