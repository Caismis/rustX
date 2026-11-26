import { useMemo, useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { Button } from '../../../../presentation/primitives/Button';
import { Input } from '../../../../presentation/primitives/Input';
import { Tooltip } from '../../../../presentation/primitives/Tooltip';
import { IconChevronRightOutline14, IconCloseOutline16, IconFolderClose16, IconSearchOutline16 } from '../../../../presentation/primitives/icons';
import type { Part } from './structure';

export interface PartsTreeLabels {
  parts: string; part: string; expand: string; collapse: string; visible: string;
  hiddenParent: string; hide: string; show: string;
  searchParts: string; clearPartsSearch: string; noPartsMatch: string;
}
interface PartsTreeProps {
  parts: readonly Part[]; selected?: string; hidden?: readonly string[];
  expanded: ReadonlySet<string>; onExpandedChange: (expanded: ReadonlySet<string>) => void;
  onSelect: (id?: string) => void; onVisibility: (id: string, visible: boolean) => void;
  labels: PartsTreeLabels;
}
interface PartBranch { part: Part; ordinal: number; children: PartBranch[] }

/** Filters admitted metadata only: at most 512 scene nodes, with 256-char names.
 * Query and forced expansion are local to this panel; scene state stays owned
 * by the supplied callbacks and saved expansion never follows a search. */
export function PartsTree({ parts, selected, hidden, expanded, onExpandedChange, onSelect, onVisibility, labels }: PartsTreeProps) {
  const [query, setQuery] = useState(''), search = useRef<HTMLDivElement>(null);
  const needle = query.trim().toLowerCase(), filtering = !!needle;
  const tree = useMemo(() => partTree(parts), [parts]);
  const shown = useMemo(() => {
    if (!needle) return tree;
    const matches = (branch: PartBranch): PartBranch[] => {
      const children = branch.children.flatMap(matches);
      return partName(branch.part, branch.ordinal, labels.part).toLowerCase().includes(needle) || children.length ? [{ ...branch, children }] : [];
    };
    return tree.flatMap(matches);
  }, [tree, needle, labels.part]);
  const renderPart = ({ part, ordinal, children }: PartBranch): ReactNode => {
    const name = partName(part, ordinal, labels.part), visible = !hidden?.includes(part.id);
    const inherited = hidden?.some(id => part.id.startsWith(`${id}.`)), open = filtering || expanded.has(part.id);
    return <li key={part.id}>
      <div className="flex items-center gap-[2px] min-w-0 rounded-[var(--dsw-radius-sm)] hover:bg-[var(--dsw-alias-interactive-bg-hover)]" data-hidden={!visible || inherited || undefined}>
        {children.length ? <Button size="sm" className="w-[20px] flex-none p-0! [color:var(--dsw-alias-label-tertiary)] [&_svg]:transition-transform [&_svg]:duration-[140ms] motion-reduce:[&_svg]:transition-none" aria-label={`${open ? labels.collapse : labels.expand}: ${name}`} aria-expanded={open} disabled={filtering} onClick={() => {
          const next = new Set(expanded); if (open) next.delete(part.id); else next.add(part.id); onExpandedChange(next);
        }}><IconChevronRightOutline14 className={open ? 'rotate-90' : undefined}/></Button> : <span className="w-[20px] flex-none"/>}
        <Button size="sm" className={clsx('justify-start min-w-0 flex-1 px-[6px] rounded-[var(--dsw-radius-sm)] text-start text-[12px] [&>span:last-child]:overflow-hidden [&>span:last-child]:text-ellipsis [&>span:last-child]:whitespace-nowrap [&>span:first-child]:flex-none [&>span:first-child]:[color:var(--dsw-alias-label-tertiary)] aria-pressed:bg-[var(--dsw-alias-interactive-bg-active)]', (!visible || inherited) && 'opacity-50')} title={name} icon={part.kind === 'group' ? <IconFolderClose16/> : <ModelGlyph/>} aria-pressed={selected === part.id} onClick={() => onSelect(selected === part.id ? undefined : part.id)}><span>{name}</span></Button>
        <Tooltip label={`${inherited ? labels.hiddenParent : visible ? labels.hide : labels.show}: ${name}`} side="top" delayMs={500}><span className="flex flex-none"><Button size="sm" className="flex-none w-[28px] p-0! [color:var(--dsw-alias-label-secondary)]" role="checkbox" aria-label={`${labels.visible}: ${name}`} aria-description={inherited ? labels.hiddenParent : undefined} aria-checked={visible} onClick={() => onVisibility(part.id, !visible)}><VisibilityGlyph visible={visible}/></Button></span></Tooltip>
      </div>
      {children.length > 0 && open && <ul>{children.map(renderPart)}</ul>}
    </li>;
  };
  return <div className="flex flex-1 flex-col min-h-0 min-w-0">
    <div className="flex flex-none items-center gap-[4px] p-[6px] [border-bottom:0.5px_solid_var(--dsw-alias-border-l3)] [&>span]:flex-1 [&>span]:min-w-0 [&>span]:h-[28px] [&_input]:w-full [&_input]:text-[12px] [&_input]:leading-[18px] [&_input::-webkit-search-cancel-button]:hidden" ref={search}>
      <Input type="search" icon={<IconSearchOutline16/>} aria-label={labels.searchParts} placeholder={labels.searchParts} maxLength={256} value={query} onChange={event => setQuery(event.target.value.slice(0, 256))}/>
      {query && <Button size="sm" className="flex-none w-[24px] h-[24px] p-0! [color:var(--dsw-alias-label-secondary)]" aria-label={labels.clearPartsSearch} onClick={() => { setQuery(''); search.current?.querySelector('input')?.focus({ preventScroll: true }); }}><IconCloseOutline16/></Button>}
    </div>
    <div className="flex-1 min-h-0 overflow-auto overscroll-contain [scrollbar-gutter:stable]">
      <ul className="list-none p-[6px] m-0 [&_ul]:list-none [&_ul]:pl-[12px] [&_ul]:m-0" aria-label={labels.parts}>{shown.map(renderPart)}</ul>
      {filtering && !shown.length && <p className="m-0 px-[12px] pt-[8px] pb-[14px] [color:var(--dsw-alias-label-tertiary)] text-[12px] leading-[18px]" role="status">{labels.noPartsMatch}</p>}
    </div>
  </div>;
}

function partName(part: Part, ordinal: number, label: string) { return part.name || `${label} ${ordinal}`; }

function partTree(parts: readonly Part[]): PartBranch[] {
  const roots: PartBranch[] = [], parents: PartBranch[] = [];
  parts.forEach((part, index) => {
    while (parents.length && !part.id.startsWith(`${parents.at(-1)!.part.id}.`)) parents.pop();
    const branch: PartBranch = { part, ordinal: index + 1, children: [] };
    (parents.at(-1)?.children ?? roots).push(branch); parents.push(branch);
  });
  return roots;
}

function ModelGlyph() {
  return <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinejoin="round" aria-hidden="true"><path d="m8 1.8 5.5 3.1v6.2L8 14.2l-5.5-3.1V4.9L8 1.8Z"/><path d="m2.5 4.9 5.5 3.2 5.5-3.2M8 8.1v6.1"/></svg>;
}
function VisibilityGlyph({ visible }: { visible: boolean }) {
  return <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M1.5 8S4 3.7 8 3.7 14.5 8 14.5 8s-2.5 4.3-6.5 4.3S1.5 8 1.5 8Z"/><circle cx="8" cy="8" r="1.8"/>{!visible && <path d="m2 2 12 12"/>}</svg>;
}
