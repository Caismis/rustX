
import { $isLinkNode, LinkNode, TOGGLE_LINK_COMMAND } from '@lexical/link';
import { $isListItemNode, INSERT_UNORDERED_LIST_COMMAND, ListItemNode, ListNode } from '@lexical/list';
import { LexicalComposer } from '@lexical/react/LexicalComposer';
import { useLexicalComposerContext } from '@lexical/react/LexicalComposerContext';
import { ContentEditable } from '@lexical/react/LexicalContentEditable';
import { LexicalErrorBoundary } from '@lexical/react/LexicalErrorBoundary';
import { HistoryPlugin } from '@lexical/react/LexicalHistoryPlugin';
import { LinkPlugin } from '@lexical/react/LexicalLinkPlugin';
import { ListPlugin } from '@lexical/react/LexicalListPlugin';
import { RichTextPlugin } from '@lexical/react/LexicalRichTextPlugin';
import { $getSelectionStyleValueForProperty, $isAtNodeEnd, $patchStyleText, $setBlocksType } from '@lexical/selection';
import { $createParagraphNode, $createTextNode, $getNodeByKey, $getRoot, $getSelection, $isParagraphNode, $isRangeSelection, FORMAT_TEXT_COMMAND, type EditorThemeClasses, type LexicalEditor, type RangeSelection } from 'lexical';
import clsx from 'clsx';
import { createContext, memo, useContext, useEffect, useRef, useState, type ReactNode } from 'react';
import type { WorkflowNote, WorkflowNoteTheme } from '../../../../../../../protocol/app-server/v80';
import { useTranslation } from '../../../../../locale/react';
import { ShortcutKeys } from '../../../../../presentation/primitives/ShortcutKeys';
import { IconCheckOutline16, IconEllipsisOutline16 } from '../../../../../presentation/primitives/icons';
import { IconBold, IconBulletList, IconExternal, IconFontSize, IconItalic, IconLink, IconPencil, IconStrikethrough, IconUnlink } from '../../icons';
import { COPY_KEYS, DELETE_KEYS, DUPLICATE_KEYS } from '../shortcuts';
import { Input as TextInput } from '@/presentation/primitives/Input';
import { Menu, MenuItem, MenuPopup, MenuSeparator, MenuTrigger } from '@/components/ui/menu';
import { Popover, PopoverPopup, PopoverTrigger } from '@/components/ui/popover';

/** What a note does to the document; the canvas owns selection and history. */
export const NoteActions = createContext<{
 editable: boolean;
 change(id: string, change: Partial<Pick<WorkflowNote, 'text' | 'theme'>>): void;
 copy(id: string): void;
 duplicate(id: string): void;
 remove(id: string): void;
}>({ editable: false, change() {}, copy() {}, duplicate() {}, remove() {} });

const NOTE_THEME_VARS: Record<string, string> = {
 blue: '[--note-bg:light-dark(#eff8ff,#102a56)] [--note-title:light-dark(#d1e9ff,#194185)] [--note-border:light-dark(#84caff,#1570ef)] [--note-outer:#2e90fa]',
 cyan: '[--note-bg:light-dark(#ecfdff,#0d2d3a)] [--note-title:light-dark(#cff9fe,#155b75)] [--note-border:light-dark(#67e3f9,#088ab2)] [--note-outer:#06aed4]',
 green: '[--note-bg:light-dark(#edfcf2,#052e1c)] [--note-title:light-dark(#d3f8df,#095c37)] [--note-border:light-dark(#73e2a3,#099250)] [--note-outer:#16b364]',
 yellow: '[--note-bg:light-dark(#fefbe8,#713b12)] [--note-title:light-dark(#fef7c3,#854a0e)] [--note-border:light-dark(#fde272,#ca8504)] [--note-outer:#eaaa08]',
 pink: '[--note-bg:light-dark(#fdf2fa,#4e0d30)] [--note-title:light-dark(#fce7f6,#851651)] [--note-border:light-dark(#faa7e0,#dd2590)] [--note-outer:#ee46bc]',
 violet: '[--note-bg:light-dark(#ece9fe,#2e125e)] [--note-title:light-dark(#ece9fe,#491c96)] [--note-border:light-dark(#c3b5fd,#7839ee)] [--note-outer:#875bf7]',
};
const THEMES: readonly WorkflowNoteTheme[] = ['blue', 'cyan', 'green', 'yellow', 'pink', 'violet'];
const FONT_SIZES = [['12px', 'small'], ['14px', 'medium'], ['16px', 'large']] as const;
const DEFAULT_FONT_SIZE = '12px';
// Dify's note link check.
const URL_PATTERN = /((([A-Za-z]{3,9}:(?:\/\/)?)(?:[-;:&=+$,\w]+@)?[A-Za-z0-9.-]+|(?:www.|[-;:&=+$,\w]+@)[A-Za-z0-9.-]+)((?:\/[+~%/.\w-]*)?\??[-+=&;%@.\w]*#?\w*))/;
const THEME: EditorThemeClasses = {
 paragraph: 'm-0 text-[12px] leading-[18px]',
 list: { ul: 'm-0 p-0 text-[12px] leading-[18px] list-disc', listitem: 'mt-0 mb-0 mr-[8px] ml-[18px]' },
 link: clsx('cursor-pointer font-medium [color:var(--dsw-alias-state-business-primary)] underline decoration-[1px] underline-offset-2', 'nodrag nopan nowheel'),
 text: { bold: 'font-medium', italic: 'italic', strikethrough: 'line-through' },
};

/** A note's text is its Lexical state as JSON. Text that is not one, as a
 * hand-written file may hold, reads as one plain paragraph rather than
 * failing the editor. */
function lexicalState(text: string) {
 try { if (JSON.parse(text)?.root) return text; } catch { /* plain text */ }
 return () => { $getRoot().clear().append($createParagraphNode().append($createTextNode(text))); };
}
function load(editor: LexicalEditor, text: string) {
 const state = lexicalState(text);
 if (typeof state === 'string') editor.setEditorState(editor.parseEditorState(state));
 else editor.update(state);
}

/** As Dify's NoteNode: a rich-text note written in place on the canvas. Its
 * text is the Lexical editor state as JSON; selecting it shows the toolbar. */
export const NoteNode = memo(function NoteNode({ id, note, selected, resizer }: { id: string; note: WorkflowNote; selected: boolean; resizer: ReactNode }) {
 const tx = useTranslation(), { editable } = useContext(NoteActions), editing = selected && editable;
 return <div className={clsx('relative flex flex-col w-full h-full box-border [border:0.5px_solid_var(--dsw-alias-border-l1)] rounded-[var(--dsw-radius-sm)] bg-(--note-bg) [color:var(--dsw-alias-label-secondary)]', NOTE_THEME_VARS[note.theme], selected && '[border-color:var(--note-border)]')}>
 <LexicalComposer initialConfig={{ namespace: 'workflow-note', nodes: [LinkNode, ListNode, ListItemNode], editorState: note.text ? lexicalState(note.text) : null, theme: THEME, editable, onError: error => { throw error; } }}>
 {resizer}
 <div className="flex-none h-[8px] rounded-t-[var(--dsw-radius-xs)] rounded-b-none bg-(--note-title) opacity-50" />
 <div className={clsx('flex-1 min-h-0 overflow-y-auto px-[12px] py-[10px]', selected && 'nodrag nopan nowheel')}>
 <div className="relative">
 <RichTextPlugin ErrorBoundary={LexicalErrorBoundary}
 contentEditable={<ContentEditable className={clsx('min-h-[18px] outline-none [caret-color:var(--dsw-alias-state-business-primary)] select-text', selected && 'cursor-text')} aria-label={tx('workflows:note.title')} spellCheck={false} />}
 placeholder={<div className="absolute top-0 left-0 text-[12px] leading-[18px] [color:var(--dsw-alias-label-tertiary)] pointer-events-none select-none">{tx('workflows:note.placeholder')}</div>} />
 </div>
 </div>
 <LinkPlugin />
 <ListPlugin />
 <HistoryPlugin />
 <Sync id={id} text={note.text} />
 {editing && <Tools id={id} theme={note.theme} />}
 </LexicalComposer>
 </div>;
});

/** Writes the editor back to the note, and takes the note's text when it
 * changes from outside, such as an undo. Echoes of its own writes, which may
 * arrive after later keystrokes, are not outside changes. An empty note
 * writes no state. */
function Sync({ id, text }: { id: string; text: string }) {
 const [editor] = useLexicalComposerContext(), actions = useContext(NoteActions), written = useRef([text]);
 useEffect(() => editor.registerUpdateListener(({ editorState, dirtyElements, dirtyLeaves }) => {
 if (!dirtyElements.size && !dirtyLeaves.size) return;
 const last = written.current.at(-1)!;
 const next = editorState.read(() => {
 const root = $getRoot(), only = root.getFirstChild();
 // Plain text shown as it was is not an edit.
 if (typeof lexicalState(last) !== 'string' && root.getTextContent() === last) return last;
 return root.getChildrenSize() <= 1 && (!only || $isParagraphNode(only) && only.isEmpty()) ? '' : JSON.stringify(editorState.toJSON());
 });
 if (next === last) return;
 written.current.push(next);
 actions.change(id, { text: next });
 }), [editor, id, actions]);
 useEffect(() => {
 const echo = written.current.indexOf(text);
 if (echo >= 0) { written.current = written.current.slice(echo); return; }
 written.current = [text];
 if (text) load(editor, text);
 else editor.update(() => { $getRoot().clear(); });
 }, [editor, text]);
 return null;
}

interface Format { bold: boolean; italic: boolean; strikethrough: boolean; bullet: boolean; fontSize: string; link?: { key: string; url: string } }

function selectedNode(selection: RangeSelection) {
 const anchor = selection.anchor.getNode(), focus = selection.focus.getNode();
 if (anchor === focus) return anchor;
 return selection.isBackward() ? $isAtNodeEnd(selection.focus) ? anchor : focus : $isAtNodeEnd(selection.anchor) ? anchor : focus;
}

/** As Dify's format detector: the marks at the caret. */
function useFormat(): Format {
 const [editor] = useLexicalComposerContext();
 const [format, setFormat] = useState<Format>({ bold: false, italic: false, strikethrough: false, bullet: false, fontSize: DEFAULT_FONT_SIZE });
 useEffect(() => editor.registerUpdateListener(({ editorState }) => editorState.read(() => {
 const selection = $getSelection();
 if (!$isRangeSelection(selection)) return;
 const node = selectedNode(selection), parent = node.getParent();
 const link = $isLinkNode(node) ? node : $isLinkNode(parent) ? parent : undefined;
 let item = node; while (!$isListItemNode(item) && item.getParent()) item = item.getParent()!;
 setFormat({
 bold: selection.hasFormat('bold'), italic: selection.hasFormat('italic'), strikethrough: selection.hasFormat('strikethrough'), bullet: $isListItemNode(item),
 fontSize: $getSelectionStyleValueForProperty(selection, 'font-size', DEFAULT_FONT_SIZE), link: link && { key: link.getKey(), url: link.getURL() },
 });
 })), [editor]);
 return format;
}

/** As Dify's NoteEditorToolbar: color, font size, marks, link, list and more. */
function Tools({ id, theme }: { id: string; theme: WorkflowNoteTheme }) {
 const tx = useTranslation(), [editor] = useLexicalComposerContext(), actions = useContext(NoteActions), format = useFormat();
 const [colors, setColors] = useState(false), [sizes, setSizes] = useState(false);
 const command = (label: string, pressed: boolean, icon: ReactNode, run: () => void) =>
 <button type="button" className={clsx('flex flex-none items-center justify-center w-[32px] h-[32px] border-0 rounded-[var(--dsw-radius-sm)] bg-none [color:var(--dsw-alias-label-tertiary)] cursor-pointer data-[popup-open]:bg-[var(--dsw-alias-interactive-bg-hover)] data-[popup-open]:[color:var(--dsw-alias-label-secondary)] hover:bg-[var(--dsw-alias-interactive-bg-hover)] hover:[color:var(--dsw-alias-label-secondary)] nt-command [.nt-command+&]:ml-[2px]', pressed && 'bg-[var(--dsw-alias-interactive-bg-active)]! [color:var(--dsw-alias-state-business-primary)]! hover:bg-[var(--dsw-alias-interactive-bg-active)]! hover:[color:var(--dsw-alias-state-business-primary)]!')} aria-label={label} title={label} aria-pressed={pressed}
 onMouseDown={event => event.preventDefault()} onClick={run}>{icon}</button>;
 const link = () => {
 if (format.link) { unlink(format.link.key); return; }
 editor.dispatchCommand(TOGGLE_LINK_COMMAND, '');
 };
 const unlink = (key: string) => editor.update(() => {
 const node = $getNodeByKey(key);
 if (!$isLinkNode(node)) return;
 for (const child of node.getChildren()) node.insertBefore(child);
 node.remove();
 });
 const bullet = () => {
 if (!format.bullet) { editor.dispatchCommand(INSERT_UNORDERED_LIST_COMMAND, undefined); return; }
 editor.update(() => { const selection = $getSelection(); if ($isRangeSelection(selection)) $setBlocksType(selection, () => $createParagraphNode()); });
 };
 const size = (value: string) => editor.update(() => { const selection = $getSelection(); if ($isRangeSelection(selection)) $patchStyleText(selection, { 'font-size': value }); });
 const sizeLabel = tx(`workflows:note.${FONT_SIZES.find(([value]) => value === format.fontSize)?.[1] ?? 'small'}`);
 return <>
 <div className={clsx('absolute top-[-41px] left-1/2 z-40 inline-flex items-center p-[2px] border-0 rounded-[var(--dsw-radius-sm)] bg-[var(--dsw-specific-menu)] shadow-[var(--dsw-elevation-prominent)] -translate-x-1/2 whitespace-nowrap', 'nodrag nopan nowheel')} role="toolbar" aria-label={tx('workflows:note.title')} onMouseDown={event => event.stopPropagation()} onClick={event => event.stopPropagation()}>
 <Popover open={colors} onOpenChange={setColors}>
 <PopoverTrigger className="flex flex-none items-center justify-center w-[32px] h-[32px] border-0 rounded-[var(--dsw-radius-sm)] bg-none [color:var(--dsw-alias-label-tertiary)] cursor-pointer data-[popup-open]:bg-[var(--dsw-alias-interactive-bg-hover)] data-[popup-open]:[color:var(--dsw-alias-label-secondary)] hover:bg-[var(--dsw-alias-interactive-bg-hover)] hover:[color:var(--dsw-alias-label-secondary)] nt-command [.nt-command+&]:ml-[2px]" aria-label={tx('workflows:note.color')} title={tx('workflows:note.color')}><span className={clsx('block w-[16px] h-[16px] box-border [border:0.5px_solid_var(--dsw-alias-border-l1)] rounded-full bg-(--note-title)', NOTE_THEME_VARS[theme])} /></PopoverTrigger>
 <PopoverPopup side="top" sideOffset={4} className="grid grid-cols-[repeat(3,32px)] gap-[2px] p-[2px] border-0 rounded-[var(--dsw-radius-sm)] bg-[var(--dsw-specific-menu)] shadow-[var(--dsw-elevation-prominent)] outline-none [&>[data-slot=popover-viewport]]:p-0">
 {THEMES.map(value => <button key={value} type="button" className={clsx('relative flex items-center justify-center w-[32px] h-[32px] border-0 rounded-[var(--dsw-radius-sm)] bg-none cursor-pointer before:content-[""] before:absolute before:w-[20px] before:h-[20px] before:box-border before:[border:1.5px_solid_var(--note-outer)] before:rounded-full before:opacity-0 hover:before:opacity-100', NOTE_THEME_VARS[value], value === theme && 'before:opacity-100!')} aria-label={tx(`workflows:note.${value}`)} title={tx(`workflows:note.${value}`)}
 onClick={() => { actions.change(id, { theme: value }); setColors(false); }}><span className="block w-[16px] h-[16px] box-border [border:0.5px_solid_var(--dsw-alias-border-l1)] rounded-full bg-(--note-title)" /></button>)}
 </PopoverPopup>
 </Popover>
 <span className="flex-none w-[1px] h-[14px] mx-[4px] bg-[var(--dsw-alias-border-l2)]" />
 <Popover open={sizes} onOpenChange={setSizes}>
 <PopoverTrigger className="flex flex-none items-center justify-center gap-[4px] h-[32px] pl-[8px] pr-[6px] border-0 rounded-[var(--dsw-radius-sm)] bg-none [color:var(--dsw-alias-label-tertiary)] text-[13px] font-medium cursor-pointer data-[popup-open]:bg-[var(--dsw-alias-interactive-bg-hover)] data-[popup-open]:[color:var(--dsw-alias-label-secondary)] hover:bg-[var(--dsw-alias-interactive-bg-hover)] hover:[color:var(--dsw-alias-label-secondary)]" aria-label={tx('workflows:note.font-size')}><IconFontSize size={16} />{sizeLabel}</PopoverTrigger>
 <PopoverPopup side="bottom" align="start" sideOffset={2} className="w-[120px] p-[4px] border-0 rounded-[var(--dsw-radius-sm)] bg-[var(--dsw-specific-menu)] [color:var(--dsw-alias-label-secondary)] shadow-[var(--dsw-elevation-prominent)] outline-none [&>[data-slot=popover-viewport]]:p-0" initialFocus={false} finalFocus={false}>
 {FONT_SIZES.map(([value, name]) => <button key={value} type="button" className="flex items-center justify-between w-full h-[32px] pr-[8px] pl-[12px] border-0 rounded-[var(--dsw-radius-sm)] bg-none [color:inherit] cursor-pointer hover:bg-[var(--dsw-alias-interactive-bg-hover)] [&_svg]:[color:var(--dsw-alias-state-business-primary)]" onMouseDown={event => event.preventDefault()} onClick={() => { size(value); setSizes(false); }}>
 <span style={{ fontSize: value }}>{tx(`workflows:note.${name}`)}</span>{format.fontSize === value && <IconCheckOutline16 />}
 </button>)}
 </PopoverPopup>
 </Popover>
 <span className="flex-none w-[1px] h-[14px] mx-[4px] bg-[var(--dsw-alias-border-l2)]" />
 {command(tx('workflows:note.bold'), format.bold, <IconBold size={16} />, () => editor.dispatchCommand(FORMAT_TEXT_COMMAND, 'bold'))}
 {command(tx('workflows:note.italic'), format.italic, <IconItalic size={16} />, () => editor.dispatchCommand(FORMAT_TEXT_COMMAND, 'italic'))}
 {command(tx('workflows:note.strikethrough'), format.strikethrough, <IconStrikethrough size={16} />, () => editor.dispatchCommand(FORMAT_TEXT_COMMAND, 'strikethrough'))}
 {command(tx('workflows:note.link'), !!format.link, <IconLink size={16} />, link)}
 {command(tx('workflows:note.bullet-list'), format.bullet, <IconBulletList size={16} />, bullet)}
 <span className="flex-none w-[1px] h-[14px] mx-[4px] bg-[var(--dsw-alias-border-l2)]" />
 <Menu modal={false}>
 <MenuTrigger className="flex flex-none items-center justify-center w-[32px] h-[32px] border-0 rounded-[var(--dsw-radius-sm)] bg-none [color:var(--dsw-alias-label-tertiary)] cursor-pointer data-[popup-open]:bg-[var(--dsw-alias-interactive-bg-hover)] data-[popup-open]:[color:var(--dsw-alias-label-secondary)] hover:bg-[var(--dsw-alias-interactive-bg-hover)] hover:[color:var(--dsw-alias-label-secondary)] nt-command [.nt-command+&]:ml-[2px]" aria-label={tx('workflows:note.more')} title={tx('workflows:note.more')}><IconEllipsisOutline16 /></MenuTrigger>
 <MenuPopup side="bottom" align="end" sideOffset={4} className="min-w-[192px] p-[4px] border-0 rounded-[var(--dsw-radius-sm)] bg-[var(--dsw-specific-menu)] [color:var(--dsw-alias-label-secondary)] shadow-[var(--dsw-elevation-prominent)] outline-none [&>div]:p-0">
 <MenuItem className="flex items-center justify-between gap-[16px] h-[32px] px-[12px] rounded-[var(--dsw-radius-sm)] text-[13px] cursor-pointer outline-none data-[highlighted]:bg-[var(--dsw-alias-interactive-bg-hover)] [&>span:last-child]:[color:var(--dsw-alias-label-tertiary)]" onClick={() => actions.copy(id)}>{tx('workflows:canvas.copy')}<ShortcutKeys keys={COPY_KEYS} /></MenuItem>
 <MenuItem className="flex items-center justify-between gap-[16px] h-[32px] px-[12px] rounded-[var(--dsw-radius-sm)] text-[13px] cursor-pointer outline-none data-[highlighted]:bg-[var(--dsw-alias-interactive-bg-hover)] [&>span:last-child]:[color:var(--dsw-alias-label-tertiary)]" onClick={() => actions.duplicate(id)}>{tx('workflows:canvas.duplicate')}<ShortcutKeys keys={DUPLICATE_KEYS} /></MenuItem>
 <MenuSeparator className="h-[1px] my-[4px] mx-[-4px] bg-[var(--dsw-alias-border-l2)]" />
 <MenuItem className={clsx('flex items-center justify-between gap-[16px] h-[32px] px-[12px] rounded-[var(--dsw-radius-sm)] text-[13px] cursor-pointer outline-none data-[highlighted]:bg-[var(--dsw-alias-interactive-bg-hover)] [&>span:last-child]:[color:var(--dsw-alias-label-tertiary)]', 'data-[highlighted]:[color:var(--dsw-alias-state-error-primary)]')} onClick={() => actions.remove(id)}>{tx('workflows:note.delete')}<ShortcutKeys keys={DELETE_KEYS} /></MenuItem>
 </MenuPopup>
 </Menu>
 </div>
 {format.link && <LinkBar key={format.link.key} link={format.link} unlink={unlink} />}
 </>;
}

/** As Dify's link editor: an empty link asks for its address; a set one
 * opens, edits or unlinks. Dismissing an empty link removes it. */
function LinkBar({ link, unlink }: { link: { key: string; url: string }; unlink: (key: string) => void }) {
 const tx = useTranslation(), [editor] = useLexicalComposerContext();
 const [editing, setEditing] = useState(!link.url), [url, setUrl] = useState(link.url), [dismissed, setDismissed] = useState(false), [invalid, setInvalid] = useState(false);
 const bar = useRef<HTMLDivElement>(null);
 const [anchorElement, setAnchorElement] = useState<HTMLElement | null>(null);
 useEffect(() => { setAnchorElement(editor.getElementByKey(link.key) as HTMLElement | null); }, [editor, link.key]);
 const dismiss = (restoreEditor = false) => { setDismissed(true); if (!link.url) unlink(link.key); if (restoreEditor) editor.focus(); };
 const save = () => {
 if (!URL_PATTERN.test(url)) { setInvalid(true); return; }
 editor.update(() => { const node = $getNodeByKey(link.key); if ($isLinkNode(node)) node.setURL(url); });
 setEditing(false);
 };
 if (dismissed) return null;
 if (!anchorElement) return null;
 return <Popover open onOpenChange={(next, details) => { if (!next) dismiss(details.reason === 'escape-key'); }}><PopoverPopup ref={bar} anchor={anchorElement} positionMethod="fixed" side="top" align="center" sideOffset={4} collisionPadding={8}
 initialFocus={false} finalFocus={() => false} className={clsx('z-60 inline-flex items-center w-max p-[4px] border-0 rounded-[var(--dsw-radius-sm)] bg-[var(--dsw-specific-menu)] shadow-[var(--dsw-elevation-prominent)] [&>[data-slot=popover-viewport]]:p-0', !editing && 'p-[2px] text-[12px] font-medium [color:var(--dsw-alias-label-tertiary)]')}>
 {editing ? <>
 <TextInput unstyled className="w-[196px] h-[24px] mr-[2px] p-[4px] border-0 bg-transparent [color:var(--dsw-alias-label-primary)] text-[13px] outline-none aria-invalid:[color:var(--dsw-alias-state-error-primary)]" value={url} autoFocus aria-label={tx('workflows:note.link')} aria-invalid={invalid || undefined} placeholder={tx('workflows:note.enter-url')} title={invalid ? tx('workflows:note.invalid-url') : undefined}
 onChange={event => { setUrl(event.target.value); setInvalid(false); }} onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); if (url) save(); } }} />
 <button type="button" className="h-[24px] px-[8px] border-0 rounded-[var(--dsw-radius-sm)] bg-[var(--dsw-alias-state-business-primary)] [color:var(--dsw-alias-label-primary-inverted)] text-[12px] font-medium cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed" disabled={!url} onClick={save}>{tx('workflows:note.ok')}</button>
 </> : <>
 <a className="flex items-center gap-[4px] h-[24px] px-[8px] rounded-[var(--dsw-radius-sm)] [color:inherit] no-underline hover:bg-[var(--dsw-alias-interactive-bg-hover)] [&_span]:max-w-[140px] [&_span]:overflow-hidden [&_span]:text-ellipsis [&_span]:whitespace-nowrap [&_span]:[color:var(--dsw-alias-state-business-primary)]" href={link.url} target="_blank" rel="noopener noreferrer"><IconExternal size={12} />{tx('workflows:note.open-link')}<span title={link.url}>{link.url}</span></a>
 <span className="flex-none w-[1px] h-[14px] mx-[4px] bg-[var(--dsw-alias-border-l2)]" />
 <button type="button" className="flex items-center gap-[4px] h-[24px] px-[8px] border-0 rounded-[var(--dsw-radius-sm)] bg-none [color:inherit] cursor-pointer hover:bg-[var(--dsw-alias-interactive-bg-hover)]" onClick={() => setEditing(true)}><IconPencil size={12} />{tx('workflows:note.edit-link')}</button>
 <button type="button" className="flex items-center gap-[4px] h-[24px] px-[8px] border-0 rounded-[var(--dsw-radius-sm)] bg-none [color:inherit] cursor-pointer hover:bg-[var(--dsw-alias-interactive-bg-hover)]" onClick={() => { unlink(link.key); setDismissed(true); }}><IconUnlink size={12} />{tx('workflows:note.unlink')}</button>
 </>}
 </PopoverPopup></Popover>;
}
