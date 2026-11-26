import { useContext, useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { useSelector } from '@xstate/react';
import type { WorkflowAnalysisQuery, WorkflowReferenceChoice } from '../../../../../../../protocol/app-server/v80';
import { useTranslation } from '../../../../../locale/react';
import { consumerAddress } from '../../../document/analysis';
import { encodeField, setField, type FieldAddress } from '../../../document/fields';
import { nodePresentation } from '../../../document/geometry';
import { blockAt, nodeAt, startOf, type BlockPath, type WorkflowDocument } from '../../../document/model';
import { IconEntry, IconPlus, KindIcon } from '../../icons';
import { DraftOwner } from './drafts';
import { useCandidateAnalysis } from './analysis';
import { editorCss } from '../editorCss';
import { Input as TextInput } from '@/presentation/primitives/Input';
import { Popover, PopoverPopup } from '@/components/ui/popover';

/** A bounded native consumer query. Titles decorate the catalog; they never
 * decide availability, compatibility or lexical reference paths. `token`
 * shows Dify's variable token and floating popup, listing only usable
 * choices; a condition's token asks for the condition-variable scope. */
export function ReferencePicker({ address, path, edit, initiallyOpen = false, onChoose, onClose, placeholder, token = false, anchor, addLabel, disabled }: {
  address: FieldAddress; path: readonly string[]; edit: (apply: (document: WorkflowDocument) => WorkflowDocument) => void;
  token?: boolean; initiallyOpen?: boolean; onChoose?: (choice: WorkflowReferenceChoice) => void; onClose?: (restoreFocus: boolean) => void; placeholder?: string; anchor?: DOMRect; addLabel?:string; disabled?:boolean;
}) {
  const tx = useTranslation(), owner = useContext(DraftOwner)!;
  const document = useSelector(owner.editor, snapshot => snapshot.context.document);
  const [open, setOpen] = useState(initiallyOpen), [search, setSearch] = useState('');
  const popup = useRef<HTMLDivElement>(null);
  const virtualAnchor = useMemo(() => anchor ? { getBoundingClientRect: () => anchor } : undefined, [anchor]);
  const root = useRef<HTMLDivElement>(null), trigger = useRef<HTMLButtonElement>(null), searchInput = useRef<HTMLInputElement>(null), id = useId();
  const field = encodeField(address);
  useEffect(() => { setOpen(initiallyOpen); setSearch(''); }, [field, initiallyOpen]);
  const wasOpen = useRef(open), returnFocus = useRef(false);
  useEffect(() => { if (wasOpen.current && !open) onClose?.(returnFocus.current); wasOpen.current = open; }, [open]);
  useEffect(() => {
    if (!open) return;
    if (!token) searchInput.current?.focus();
    if (token) return;
    const outside = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node) && !popup.current?.contains(event.target as Node)) { returnFocus.current = false; setOpen(false); } };
    window.document.addEventListener('pointerdown', outside, true);
    return () => window.document.removeEventListener('pointerdown', outside, true);
  }, [open, token]);
  const titleOf = (choice: WorkflowReferenceChoice) => choice.producer && document ? nodePresentation(document, choice.producer.block, choice.producer.node)?.title || choice.path[0]
    : choice.path[0] === 'args' ? startTitle() : choice.path[0];
  const query = useMemo<WorkflowAnalysisQuery | undefined>(() => {
    const original = document && consumerAddress(document, address);
    // A condition's token lists every comparable variable, whatever the
    // other side holds now.
    const variableSide = original?.path?.at(-1)?.kind==='left'||original?.path?.at(-1)?.kind==='value';
    const consumer = original && token && variableSide && original.field.kind === 'condition' ? { ...original, field: { ...original.field, kind: 'condition_variable' as const } }
      : original && token && variableSide && original.field.kind === 'break_condition' ? { ...original, field: { kind: 'break_condition_variable' as const } } : original;
    return consumer ? { consumer, reference: path.length ? [...path] : undefined, search, limit: 128 } : undefined;
  }, [document, field, path, search, token]);
  const result = useCandidateAnalysis(query, open && !!query), report = result?.report?.analysis;
  const groups = new Map<string, WorkflowReferenceChoice[]>();
  for (const choice of report?.references ?? []) {
    // The native entry scope excludes this node, later nodes and sibling
    // branches. Keep the picker limited to sources that may exist here.
    if (choice.availability === 'unavailable') continue;
    if (token && choice.compatibility === 'incompatible') continue;
    const key = choice.path[0], group = groups.get(key) ?? [];
    group.push(choice); groups.set(key, group);
  }
  const close = () => { returnFocus.current = true; setOpen(false); trigger.current?.focus(); };
  // The workflow input belongs to the root Start, also inside a body.
  const startTitle = () => { const start = document && startOf(document.block); return (start && document && nodePresentation(document, [], start)?.title) || tx('workflows:canvas.start-title'); };
  // A producer is in this block, a block around it, or the body this
  // node's round fields read.
  const owning = (id: string): BlockPath | undefined => document ? [...(address.node ? [[...address.block, { node: address.node }]] : []), ...address.block.map((_, depth) => address.block.slice(0, address.block.length - depth)), []]
    .find(at => { const owner = blockAt(document, at); return owner && nodeAt(owner, id); }) : undefined;
  const producer = path[0], producerBlock = producer ? owning(producer) : undefined;
  const currentTitle = producer === 'args' ? startTitle() : document && producerBlock ? nodePresentation(document, producerBlock, producer)?.title || producer : producer;
  const currentNode = document && producerBlock ? nodeAt(blockAt(document, producerBlock)!, producer) : undefined;
  const currentValue = path.length > 1 ? path.slice(1).join(' / ') : tx('workflows:value.whole');
  const panelBody = <>
      <TextInput ref={searchInput} className={editorCss.input} aria-label={tx('workflows:value.search')} placeholder={tx('workflows:value.search')} value={search} onChange={event => setSearch(event.target.value)} />
      {!result && <p role="status" className={editorCss.note}>{tx('workflows:value.analyzing')}</p>}
      {result?.failed && <p role="alert" className={editorCss.note}>{tx('workflows:value.analysis-failed')}</p>}
      {report?.diagnostic && <p className={editorCss.note}>{report.diagnostic}</p>}
      {!token && report?.selected && <p className={editorCss.note}>{tx(`workflows:value.${report.selected.availability}`)} · {tx(`workflows:value.${report.selected.compatibility}`)}</p>}
      {[...groups].map(([producer, choices]) => <fieldset className={editorCss.variableGroup} key={producer}>
        <legend>{titleOf(choices[0])}<code>{producer}</code></legend>
        {choices.map(choice => {
          const label = choice.path.length > 1 ? choice.path.slice(1).join(' / ') : tx('workflows:value.whole');
          const type = typeof choice.schema?.type === 'string' ? choice.schema.type : tx('workflows:value.unknown-type');
          return <button type="button" data-variable className={editorCss.variableChoice} key={JSON.stringify(choice.path)}
            aria-label={`${choice.path.join(' / ')} · ${tx(`workflows:value.${choice.availability}`)} · ${tx(`workflows:value.${choice.compatibility}`)}`}
            disabled={choice.availability === 'unavailable' || choice.compatibility === 'incompatible'} title={choice.diagnostic ?? undefined}
            onClick={() => { if (onChoose) onChoose(choice); else edit(document => setField(document, address, { type: 'reference', path: choice.path })); close(); }}>
            <span>{label} <code>{type}</code></span>{(!token || choice.availability === 'unavailable' || choice.compatibility === 'incompatible') && <small>{tx(`workflows:value.${choice.availability}`)} · {tx(`workflows:value.${choice.compatibility}`)}</small>}
            {choice.diagnostic && <small>{choice.diagnostic}</small>}
          </button>;
        })}
      </fieldset>)}
      {report && !groups.size && <p className={editorCss.note}>{tx('workflows:value.no-matches')}</p>}
      {report?.truncated && <p className={editorCss.note}>{tx('workflows:value.truncated')}</p>}
      <button type="button" className={editorCss.smallButton} onClick={close}>{tx('workflows:value.close-picker')}</button>
  </>;
  const panelKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.nativeEvent.isComposing) return;
    if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(); }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      const choices = [...popup.current!.querySelectorAll<HTMLButtonElement>('[data-variable]:not(:disabled)')], active = choices.indexOf(window.document.activeElement as HTMLButtonElement);
      const next = event.key === 'ArrowDown' ? Math.min(active + 1, choices.length - 1) : Math.max(active - 1, 0);
      if (choices[next]) { event.preventDefault(); choices[next].focus(); }
    }
  };
  return <Popover open={token && open} onOpenChange={(next, details) => { if (!next) { returnFocus.current = details.reason === 'escape-key'; setOpen(false); } }}><div ref={root} className={editorCss.referencePicker}>
    <button style={anchor?{display:'none'}:undefined} hidden={!!anchor} ref={trigger} data-workflow-field={field} type="button" disabled={disabled} title={path.length?`${currentTitle} / ${currentValue}`:undefined} className={addLabel ? editorCss.iconButton : token ? `${editorCss.referenceTrigger} ${editorCss.branchVariableTrigger}` : editorCss.referenceTrigger} aria-label={addLabel??tx('workflows:value.pick')} aria-haspopup="dialog" aria-expanded={open} aria-controls={id} onClick={() => { returnFocus.current = open; setOpen(value => !value); }}>
      {addLabel?<IconPlus size={14}/>:token && path.length ? <span className={editorCss.branchVariableToken}>
        {producer === 'args' ? <IconEntry size={13} /> : currentNode ? <KindIcon kind={currentNode.type} size={13} /> : null}
        <span className={editorCss.branchVariableSource}>{currentTitle}</span><span className={editorCss.branchVariableSlash}>/</span><b><span aria-hidden>{'{x}'}</span> {currentValue}</b>
      </span> : <span>{currentTitle || placeholder || tx('workflows:value.pick')}{path.length > 1 ? ` / ${path.slice(1).join(' / ')}` : ''}</span>}
    </button>
    {open && (token
      ? <PopoverPopup ref={popup} id={id} className={`${editorCss.referencePopup} ${editorCss.branchVariablePopup}`} role="dialog" aria-label={tx('workflows:value.pick')}
        anchor={virtualAnchor ?? trigger} positionMethod="fixed" side="bottom" align="start" sideOffset={6} collisionPadding={12} initialFocus={searchInput} finalFocus={() => false}
        onWheel={event => event.stopPropagation()} onKeyDown={panelKeyDown}>{panelBody}</PopoverPopup>
      : <div ref={popup} id={id} className={editorCss.referencePopup} role="dialog" aria-label={tx('workflows:value.pick')} onWheel={event => event.stopPropagation()} onKeyDown={panelKeyDown}>{panelBody}</div>)}
  </div></Popover>;
}
