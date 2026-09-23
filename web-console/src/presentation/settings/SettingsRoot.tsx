/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
import { useLayoutEffect, useRef, useSyncExternalStore, type ReactNode } from 'react';
import { Dialog, Modal, ModalOverlay, Tab, TabList, TabPanel, Tabs } from 'react-aria-components';
import clsx from 'clsx';
import { IconCloseOutline16, IconSettingsOutline16 } from '../primitives/icons';
import css from './SettingsRoot.module.css';
import workflow from './SettingsWorkflow.module.css';

/** The viewport at which the Settings panel becomes a single column with a
 * horizontal page strip. SettingsRoot.module.css sizes the narrow panel at the
 * same width; the strip itself is laid out from the tab orientation. */
const narrowSettingsLayout = '(max-width: 640px)';

/** Whether a media query matches, as an external store: the browser owns the
 * viewport, and every change re-renders synchronously with it. An environment
 * without media queries has no narrow viewport. */
function useMediaQuery(query: string): boolean {
  return useSyncExternalStore(
    notify => {
      const list = window.matchMedia?.(query);
      list?.addEventListener('change', notify);
      return () => list?.removeEventListener('change', notify);
    },
    () => window.matchMedia?.(query).matches ?? false,
    () => false,
  );
}

/**
 * The one Settings modal root: full-viewport mask + centered panel, with the
 * primary product pages as its tab list.
 *
 * Focus containment, focus restoration to the trigger, Escape and the
 * dismissable mask are React Aria's, not hand-written: `ModalOverlay`/`Modal`/
 * `Dialog` own the dialog semantics and `Tabs`/`TabList`/`Tab`/`TabPanel` own
 * roving focus and arrow-key page navigation. The visual result is the same
 * Harness-derived panel, rail and cell geometry as before.
 *
 * The page list is a vertical rail on a wide viewport and a horizontal strip on
 * a narrow one. Its React Aria orientation is chosen from the viewport, and its
 * layout is styled from that orientation, so ArrowUp/ArrowDown move along the
 * rail and ArrowLeft/ArrowRight along the strip — keyboard semantics and the
 * visual axis are one decision.
 *
 * There is exactly one of these in the application. A confirmation inside a
 * page opens its own transient layer over this one; it never builds a second
 * Settings tree.
 */
export function SettingsPanel({ pages, activeId, onSelect, onClose, children, actions }: {
  /** `activeId` is always one of `pages`: the owner of navigation admits no
   * other, so it is rendered as it is. */
  pages: readonly { id: string; label: string }[]; activeId: string; onSelect: (id: string) => void;
  onClose: () => void; children: ReactNode; actions?: ReactNode;
}) {
  const orientation = useMediaQuery(narrowSettingsLayout) ? 'horizontal' : 'vertical';
  return (
    <ModalOverlay className={css.overlay} isOpen isDismissable onOpenChange={open => { if (!open) onClose(); }}>
      <Modal className={css.panel}>
        <Dialog className={css.dialog} aria-label="Settings">
          <Tabs className={css.tabsRoot} orientation={orientation} selectedKey={activeId} onSelectionChange={key => { onSelect(String(key)); }}>
            <nav className={css.nav}>
              <div className={css.navTitle}>Settings</div>
              <TabList className={clsx(css.navList, workflow.pageTabs)} aria-label="Settings pages">
                {pages.map(page => (
                  <Tab key={page.id} id={page.id} className={clsx(css.navCell, workflow.pageTab)}>
                    {({ isSelected }) => <>
                      <IconSettingsOutline16 className={css.navIcon} size={16} />
                      <PageTabLabel label={page.label} selected={isSelected} orientation={orientation} />
                    </>}
                  </Tab>
                ))}
              </TabList>
            </nav>
            <div className={css.content}>
              <div className={css.header}>
                <div className={css.actions}>{actions}</div>
                <button type="button" className={css.close} onClick={onClose}>
                  <IconCloseOutline16 size={14} />
                  <span className={css.hiddenLabel}>Close Settings</span>
                </button>
              </div>
              <div className={css.options}>
                <TabPanel key={activeId} id={activeId} className={workflow.pageTabPanel}>{children}</TabPanel>
              </div>
            </div>
          </Tabs>
        </Dialog>
      </Modal>
    </ModalOverlay>
  );
}

/** One page tab's label, which keeps a selected tab inside a horizontal strip.
 *
 * A strip can overflow, and selecting a page by pointer, by focus restoration or
 * by a navigation decision that does not move focus (opening Connection) does
 * not scroll it. The selected tab is brought into view horizontally, by the
 * minimum distance, once it is committed as selected — which is why this lives
 * on the tab rather than on the list: the list commits before React Aria has
 * rendered its collection. */
function PageTabLabel({ label, selected, orientation }: { label: string; selected: boolean; orientation: 'horizontal' | 'vertical' }) {
  const text = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => {
    const tab = text.current?.closest<HTMLElement>('[role="tab"]');
    const strip = tab?.closest<HTMLElement>('[role="tablist"]');
    if (!selected || orientation !== 'horizontal' || !tab || !strip || strip.scrollWidth <= strip.clientWidth) return;
    const bounds = strip.getBoundingClientRect(), cell = tab.getBoundingClientRect();
    if (cell.left < bounds.left) strip.scrollLeft -= bounds.left - cell.left;
    else if (cell.right > bounds.right) strip.scrollLeft += cell.right - bounds.right;
  }, [selected, orientation]);
  return <span ref={text} className={clsx(css.navLabel, workflow.pageTabLabel)}>{label}</span>;
}

export function SettingsTrigger({ wide, onClick }: { wide: boolean; onClick: () => void }) {
  return <div className={clsx(css.triggerRow, !wide && css.railRow)}><button type="button" className={clsx(css.trigger, !wide && css.rail)} aria-label="Settings" aria-haspopup="dialog" onClick={onClick}>
    <IconSettingsOutline16 />{wide && <span className={css.triggerLabel}>Settings</span>}
  </button></div>;
}
