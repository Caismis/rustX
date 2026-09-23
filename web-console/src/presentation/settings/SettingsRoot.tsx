/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
import { useLayoutEffect, useRef, type ReactNode } from 'react';
import { Dialog, Modal, ModalOverlay, Tab, TabList, TabPanel, Tabs } from 'react-aria-components';
import clsx from 'clsx';
import { IconCloseOutline16, IconSettingsOutline16 } from '../primitives/icons';
import css from './SettingsRoot.module.css';
import workflow from './SettingsWorkflow.module.css';

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
 * There is exactly one of these in the application. A confirmation inside a
 * page opens its own transient layer over this one; it never builds a second
 * Settings tree.
 */
export function SettingsPanel({ pages, activeId, onSelect, onClose, children, actions }: {
  pages: readonly { id: string; label: string }[]; activeId: string; onSelect: (id: string) => void;
  onClose: () => void; children: ReactNode; actions?: ReactNode;
}) {
  // Entries can unmount underneath the requested id, so the render-time
  // projection falls back to the first page when the id is gone.
  const active = pages.find(page => page.id === activeId)?.id ?? pages[0]?.id ?? '';
  const list = useRef<HTMLDivElement>(null);
  // On a narrow viewport the page tabs are a horizontal strip. Selecting a page
  // by pointer or by focus restoration does not scroll it, so the selected page
  // could sit outside the strip; it is brought into view horizontally, by the
  // minimum distance, whenever the selection changes.
  useLayoutEffect(() => {
    const strip = list.current;
    const tab = strip?.querySelector<HTMLElement>('[aria-selected="true"]');
    if (!strip || !tab || strip.scrollWidth <= strip.clientWidth) return;
    const bounds = strip.getBoundingClientRect(), cell = tab.getBoundingClientRect();
    if (cell.left < bounds.left) strip.scrollLeft -= bounds.left - cell.left;
    else if (cell.right > bounds.right) strip.scrollLeft += cell.right - bounds.right;
  }, [active]);
  return (
    <ModalOverlay className={css.overlay} isOpen isDismissable onOpenChange={open => { if (!open) onClose(); }}>
      <Modal className={css.panel}>
        <Dialog className={css.dialog} aria-label="Settings">
          <Tabs className={css.tabsRoot} orientation="vertical" selectedKey={active} onSelectionChange={key => { onSelect(String(key)); }}>
            <nav className={css.nav}>
              <div className={css.navTitle}>Settings</div>
              <TabList ref={list} className={clsx(css.navList, workflow.pageTabs)} aria-label="Settings pages">
                {pages.map(page => (
                  <Tab key={page.id} id={page.id} className={clsx(css.navCell, workflow.pageTab)}>
                    <IconSettingsOutline16 className={css.navIcon} size={16} />
                    <span className={clsx(css.navLabel, workflow.pageTabLabel)}>{page.label}</span>
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
                <TabPanel key={active} id={active} className={workflow.pageTabPanel}>{children}</TabPanel>
              </div>
            </div>
          </Tabs>
        </Dialog>
      </Modal>
    </ModalOverlay>
  );
}

export function SettingsTrigger({ wide, onClick }: { wide: boolean; onClick: () => void }) {
  return <div className={clsx(css.triggerRow, !wide && css.railRow)}><button type="button" className={clsx(css.trigger, !wide && css.rail)} aria-label="Settings" aria-haspopup="dialog" onClick={onClick}>
    <IconSettingsOutline16 />{wide && <span className={css.triggerLabel}>Settings</span>}
  </button></div>;
}
