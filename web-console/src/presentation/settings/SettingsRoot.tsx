/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
import { useRef, useState, type ReactNode } from 'react';
import { Dialog, Modal, ModalOverlay, Tab, TabList, TabPanel, Tabs } from 'react-aria-components';
import clsx from 'clsx';
import { Menu } from '../primitives/Menu';
import { IconChevronDownOutline14, IconCloseOutline16, IconSettingsOutline16 } from '../primitives/icons';
import css from './SettingsRoot.module.css';
import workflow from './SettingsWorkflow.module.css';

/** One primary Settings page as the panel presents it. The icon is a
 * recognition aid only: the label is always the page's accessible name. */
export interface SettingsPageEntry { id: string; label: string; icon: ReactNode }

/**
 * The one Settings modal root: full-viewport mask + centered panel, with the
 * primary product pages as its navigation.
 *
 * Focus containment, focus restoration to the trigger, Escape and the
 * dismissable mask are React Aria's, not hand-written: `ModalOverlay`/`Modal`/
 * `Dialog` own the dialog semantics and `Tabs`/`TabList`/`Tab`/`TabPanel` own
 * roving focus and arrow-key page navigation along the vertical rail.
 *
 * The panel has two layouts and no layout state. It is a size container, and
 * `SettingsRoot.module.css` decides from its own usable inline size — never
 * from the browser window — whether the rail or the section menu is shown: a
 * narrow panel hides the rail and shows a section menu in its header instead.
 * Both are always rendered, so no JavaScript breakpoint, media-query store or
 * resize observer takes part, and whichever one CSS hides is out of the
 * accessibility tree and the tab order. The section menu is the shared rustX
 * `Menu`, portaled and placed by Floating UI; like the rail, it only asks the
 * owner of navigation to select a page. Its open/closed state is the one
 * transient fact this component holds. When the panel widens while that menu
 * is open, its trigger leaves layout and the `Menu` closes itself through
 * `onClose`, so the open state never outlives the layout that shows it and
 * this component never mirrors the container query. The dialog is the menu's
 * named focus owner: a keyboard the menu held stays in Settings, on the
 * dialog, exactly where a freshly opened Settings starts, and Tab continues
 * from there into the header and the rail.
 *
 * There is exactly one of these in the application. A confirmation inside a
 * page opens its own transient layer over this one; it never builds a second
 * Settings tree.
 */
export function SettingsPanel({ pages, activeId, onSelect, onClose, context, children }: {
  /** `activeId` is always one of `pages`: the owner of navigation admits no
   * other, so it is rendered as it is. */
  pages: readonly SettingsPageEntry[]; activeId: string; onSelect: (id: string) => void;
  /** The owner and observation state of the open Settings instance. It sits in
   * the fixed header, so every page shares it and none of it scrolls away. */
  context?: ReactNode;
  onClose: () => void; children: ReactNode;
}) {
  const [sections, setSections] = useState(false);
  const dialogRef = useRef<HTMLElement>(null);
  const active = pages.find(page => page.id === activeId)!;
  return (
    <ModalOverlay className={css.overlay} isOpen isDismissable onOpenChange={open => { if (!open) onClose(); }}>
      <Modal className={css.panel}>
        <Dialog ref={dialogRef} className={css.dialog} aria-label="Settings">
          {/* The header is deliberately outside <Tabs>: React Aria renders a
              Tabs subtree a second time into a detached collection document
              to discover its tabs, and the section menu measures real DOM. */}
          <header className={css.header}>
            <div className={css.sections}>
              <Menu open={sections} onClose={() => setSections(false)} autoFocus focusOwner={dialogRef}
                items={pages.map(page => ({ id: page.id, label: page.label, icon: page.icon }))} selectedId={activeId}
                onSelect={id => { setSections(false); onSelect(id); }}
                anchor={<button type="button" className={css.sectionTrigger} aria-haspopup="menu" aria-expanded={sections}
                  aria-label={`Settings page: ${active.label}`} onClick={() => setSections(open => !open)}>
                  <span className={css.navIcon} aria-hidden="true">{active.icon}</span>
                  <span className={css.sectionLabel}>{active.label}</span>
                  <IconChevronDownOutline14 className={css.sectionChevron} />
                </button>} />
            </div>
            <div className={css.context}>{context}</div>
            <button type="button" className={css.close} onClick={onClose}>
              <IconCloseOutline16 size={14} />
              <span className={css.hiddenLabel}>Close Settings</span>
            </button>
          </header>
          <Tabs className={css.tabsRoot} orientation="vertical" selectedKey={activeId} onSelectionChange={key => { onSelect(String(key)); }}>
            <nav className={css.nav} aria-label="Settings navigation">
              <div className={css.navTitle}>Settings</div>
              <TabList className={clsx(css.navList, workflow.pageTabs)} aria-label="Settings pages">
                {pages.map(page => (
                  <Tab key={page.id} id={page.id} className={clsx(css.navCell, workflow.pageTab)}>
                    <span className={css.navIcon} aria-hidden="true">{page.icon}</span>
                    <span className={clsx(css.navLabel, workflow.pageTabLabel)}>{page.label}</span>
                  </Tab>
                ))}
              </TabList>
            </nav>
            <div className={css.options}>
              <TabPanel key={activeId} id={activeId} className={workflow.pageTabPanel}>{children}</TabPanel>
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
