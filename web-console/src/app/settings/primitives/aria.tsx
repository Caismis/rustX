import { useTranslation } from '../../../locale/react';
import { useId, useRef, useState, type ReactNode, type RefObject } from 'react';
import { DialogSurface } from '../../../presentation/primitives/DialogSurface';
import {
  Collapsible,
  CollapsiblePanel,
  CollapsibleTrigger,
} from '@/components/ui/collapsible';
import {
  Select as CossSelect,
  SelectItem,
  SelectPopup,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Tabs as CossTabs, TabsContent, TabsList, TabsTab } from '@/components/ui/tabs';
import { GridList, GridListItem, Heading } from 'react-aria-components';
import { IconTrashOutline16 } from '../../../presentation/primitives/icons';
import { Button } from '../../../presentation/primitives/Button';
import { Input } from '../../../presentation/primitives/Input';
import { Menu } from '../../../presentation/primitives/Menu';
import { Switch } from '../../../presentation/primitives/Switch';
import { settingsWorkflowCss } from '../../../presentation/settings/settingsWorkflowCss';
import contentCss from '../../../presentation/settings/SettingsContent.module.css';

/** coss ui owns interaction and accessibility semantics here and nothing else:
 * keyboard navigation, menu/list/select interaction, accessible tabs,
 * disclosure behavior and boolean controls. Native remains the authority on
 * resource identity and mutations; these wrappers only keep repeated Settings
 * interactions consistent. */

/** A closed enum. Native remains the authority on which values are legal; this
 * control only presents the ones the call site offers. */
export function Choice<T extends string>({ label, value, onChange, options, disabled = false, disabledKeys, description, placeholder, hideLabel = false }: {
  label: string; value: T | null; onChange: (value: T) => void;
  options: readonly (readonly [T, string])[]; disabled?: boolean; disabledKeys?: readonly T[]; description?: ReactNode; placeholder?: string;
  /** Inside a `SettingsRow` the row's own label is the visible one; the
   * control keeps it only as its accessible name. */
  hideLabel?: boolean;
}) {
  const disabledSet = new Set(disabledKeys ?? []);
  const labelId = useId();
  return <div className={settingsWorkflowCss.select}>
    {!hideLabel && <span id={labelId}>{label}</span>}
    <CossSelect
      items={options.map(([optionValue, optionLabel]) => ({ value: optionValue, label: optionLabel }))}
      value={value}
      disabled={disabled}
      onValueChange={next => { if (next !== null) onChange(next as T); }}
    >
      <SelectTrigger aria-label={label}>
        <SelectValue placeholder={placeholder} />
      </SelectTrigger>
      <SelectPopup>
        {options.map(([key, text]) => <SelectItem key={key} value={key} disabled={disabledSet.has(key)}>{text}</SelectItem>)}
      </SelectPopup>
    </CossSelect>
    {description && <span className={contentCss.hint}>{description}</span>}
  </div>;
}
/** A resource search box. Filtering is presentation; the identities it filters
 * are always the native ones the page was given. */
export function Search({ label, value, onChange, placeholder }: {
  label: string; value: string; onChange: (value: string) => void; placeholder?: string;
}) {
  return <div className={settingsWorkflowCss.search}>
    <Input value={value} onChange={event => onChange(event.currentTarget.value)} placeholder={placeholder} aria-label={label} type="search" />
  </div>;
}

/** One row of a resource list. Identity and the native facts about it are
 * separate fields on purpose: a list never collapses kind, ownership, validity,
 * preparation and availability into one badge. */
export interface ResourceRow {
  readonly id: string;
  readonly name: string;
  /** The native facts, each rendered as its own element. */
  readonly facts: ReactNode;
  readonly detail?: ReactNode;
  /** Contextual actions. Each submits through the same native mutation
   * identity and transaction owner as its page's primary editor. */
  readonly actions?: readonly RowAction[];
}
export interface RowAction { readonly id: string; readonly label: string; readonly danger?: boolean; readonly run: () => void }

/** A resource list with pointer and keyboard parity. */
export function ResourceList({ label, rows, selected, onOpen, empty }: {
  label: string; rows: readonly ResourceRow[]; selected?: string;
  onOpen: (id: string) => void; empty?: ReactNode;
}) {
  const tx = useTranslation();
  if (!rows.length) return <p className={settingsWorkflowCss.empty}>{empty ?? tx('settings:copy.no-matching-definitions-in-this-native-projection')}</p>;
  return <GridList className={settingsWorkflowCss.list} aria-label={label} selectionMode="single"
    selectedKeys={selected ? [selected] : []} onAction={key => onOpen(String(key))}>
    {rows.map(row => <GridListItem key={row.id} id={row.id} className={settingsWorkflowCss.row} textValue={row.name} aria-label={row.name}>
      <div className={settingsWorkflowCss.rowTop}>
        <span className={settingsWorkflowCss.rowName}>{row.name}</span>
        {!!row.actions?.length && <RowActions label={tx('settings:aria.actions-for-value', { p0: row.name })} actions={row.actions} />}
      </div>
      <span className={settingsWorkflowCss.rowFacts}>{row.facts}</span>
      {row.detail}
    </GridListItem>)}
  </GridList>;
}

/** The contextual action menu of one resource. */
export function RowActions({ label, actions }: { label: string; actions: readonly RowAction[] }) {
  const [open, setOpen] = useState(false);
  return <Menu open={open} anchor={<Button aria-label={label} variant="ghost" size="sm" onClick={() => setOpen(true)}>⋯</Button>} onClose={() => setOpen(false)}
    onSelect={id => { setOpen(false); actions.find(action => action.id === id)?.run(); }}
    items={actions.map(action => ({ id: action.id, label: action.label, danger: action.danger }))} />;
}

/** An advanced or secondary section. Progressive disclosure is how Settings
 * keeps advanced detail reachable without creating another primary page. */
export function Advanced({ title, children, expanded }: { title: string; children: ReactNode; expanded?: boolean }) {
  return <Collapsible className={settingsWorkflowCss.disclosure} defaultOpen={expanded}>
    <Heading level={4} style={{ margin: 0 }}>
      <CollapsibleTrigger className={settingsWorkflowCss.disclosureButton}><span className={settingsWorkflowCss.disclosureMarker} aria-hidden="true">▸</span>{title}</CollapsibleTrigger>
    </Heading>
    <CollapsiblePanel keepMounted className={settingsWorkflowCss.disclosurePanel}>{children}</CollapsiblePanel>
  </Collapsible>;
}

/** A boolean enablement control. */
export function Toggle({ label, checked, onChange, disabled = false }: {
  label: string; checked: boolean; onChange: (value: boolean) => void; disabled?: boolean;
}) {
  // Row clicks actuate the toggle, as the pre-coss React Aria label root did:
  // a wrapping label would fold its text into the switch's accessible name.
  return <div className={settingsWorkflowCss.switchRow} onClick={event => {
    if ((event.target as HTMLElement).closest('[role="switch"]') !== null) return
    if (!disabled) onChange(!checked)
  }}>
    <span>{label}</span>
    <Switch checked={checked} disabled={disabled} onChange={onChange} label={label} className={settingsWorkflowCss.switch} />
  </div>;
}

/** A confirmation of one native removal. DialogSurface owns the close-focus
 * lifecycle; `tone` distinguishes irreversible deletion from restore. */
export function ConfirmAction({ label, triggerText, title, description, confirm, onConfirm, tone, settle, disabled = false, renderTrigger }: {
  label: string; triggerText?: string; title: string; description: ReactNode; confirm: string;
  onConfirm: () => void; tone: 'destructive' | 'restore';
  /** The enabled element of the caller's workflow that takes focus after a
   * confirmation: one that the confirmed action neither disables nor
   * removes. */
  settle: RefObject<HTMLElement | null>;
  disabled?: boolean;
  renderTrigger?: (open: () => void, ref: RefObject<HTMLButtonElement | null>) => ReactNode;
}) {
  const tx = useTranslation();
  const trigger = useRef<HTMLButtonElement>(null);
  const cancel = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const closing = useRef<'dismissed' | 'confirmed'>('dismissed');
  const destructive = tone === 'destructive';
  const show = () => { closing.current = 'dismissed'; setOpen(true); };
  return <span className={settingsWorkflowCss.removal} data-tone={tone}>
    {renderTrigger ? renderTrigger(show, trigger) : <Button ref={trigger} disabled={disabled} aria-label={label}
      variant={destructive ? 'danger-outline' : 'outline'} size="sm" className={destructive ? settingsWorkflowCss.dangerTrigger : undefined}
      icon={destructive ? <IconTrashOutline16 /> : undefined} onClick={show}>
      {triggerText ?? label}
    </Button>}
    <DialogSurface open={open} onClose={() => setOpen(false)} title={title}
      role={destructive ? 'alertdialog' : 'dialog'} overlayClassName={settingsWorkflowCss.confirmOverlay}
      panelClassName={settingsWorkflowCss.confirmModal} className={settingsWorkflowCss.confirmDialog} initialFocus={cancel}
      finalFocus={() => {
        const targets = closing.current === 'confirmed' ? [settle.current] : [trigger.current, settle.current];
        // A trigger hidden out of layout (e.g. a responsive navigation swap)
        // cannot take the keyboard: viability includes being rendered.
        // A trigger hidden out of layout (e.g. a responsive navigation swap)
        // cannot take the keyboard: viability includes being rendered. The
        // rect probe applies only where layout measurement exists at all --
        // environments without layout (jsdom) report zero rects for every
        // element, which would dead-end the restoration.
        const measurable = document.documentElement.getClientRects().length > 0;
        return targets.find(target => target?.isConnected && !target.matches(':disabled, [aria-disabled="true"]') && (!measurable || target.getClientRects().length > 0)) ?? false;
      }}>
      <h2>{title}</h2>
      {description}
      <div className={settingsWorkflowCss.confirmActions}>
        <Button type="button" ref={cancel} variant="outline" onClick={() => setOpen(false)}>{tx('settings:aria.cancel')}</Button>
        <Button type="button" variant={destructive ? 'danger' : 'primary'}
          onClick={() => { closing.current = 'confirmed'; setOpen(false); onConfirm(); }}>{confirm}</Button>
      </div>
    </DialogSurface>
  </span>;
}

/** A secondary filter strip, used by the one Extensions resource surface. */
export function FilterTabs<T extends string>({ label, value, onChange, options, children }: {
  label: string; value: T; onChange: (value: T) => void;
  options: readonly (readonly [T, string])[]; children: ReactNode;
}) {
  return <CossTabs value={value} onValueChange={next => onChange(String(next) as T)}>
    <TabsList activateOnFocus className={settingsWorkflowCss.filterTabs} aria-label={label}>
      {options.map(([key, text]) => <TabsTab key={key} value={key} className={settingsWorkflowCss.filterTab}>{text}</TabsTab>)}
    </TabsList>
    <TabsContent value={value} className={settingsWorkflowCss.filterTabPanel}>{children}</TabsContent>
  </CossTabs>;
}
