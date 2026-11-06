import { useContext, useEffect, useRef, useState } from 'react';
import { Tabs } from '@base-ui/react/tabs';
import { useTranslation } from '../../../../../../locale/react';
import { Button } from '../../../../../../presentation/primitives/Button';
import { JsonCodeEditor } from '../../../../../../presentation/primitives/JsonCodeEditor';
import { Modal } from '../../../../../../presentation/primitives/Modal';
import { IconCheckOutline16, IconCloseOutline16, IconCopyOutline16, IconListPenOutline16 } from '../../../../../../presentation/primitives/icons';
import { IconBraces } from '../../../icons';
import { readJson } from '../../../../../../../shared/json-numbers';
import { fieldValue, type FieldAddress } from '../../../../document/fields';
import { CommandRejected } from '../../../../document/commands';
import { sameBlockPath, sameDocument, type WorkflowDocument } from '../../../../document/model';
import { WorkflowAnalysisSession, type AnalysisInput } from '../../../../document/analysis';
import { schemaObject } from '../../../../document/schema';
import { emptySchema, readSchema, replaceSchema, schemaAt, schemaFromExample, type SchemaEdit } from './schema';
import { VisualEditor } from './VisualEditor';
import { DraftOwner } from '../drafts';
import css from './StructuredOutput.module.css';

/** A dialog owns its working copy. Only Save sends one document edit; Escape,
 * Cancel and the close button discard everything in this editing session. */
/** `root` names the edited object as downstream references reach it. */
export function StructuredOutputDialog({ document: original, address, root, edit, onClose }: {
  document: WorkflowDocument; address: FieldAddress; root: string; edit: (apply: SchemaEdit) => void; onClose: () => void;
}) {
  const tx = useTranslation();
  const owner = useContext(DraftOwner)!;
  const validation = useRef(new WorkflowAnalysisSession()), input = useRef<AnalysisInput | undefined>(undefined);
  const [saving, setSaving] = useState(false);
  useEffect(() => () => { input.current = undefined; validation.current.cancel(); }, []);
  const [base] = useState(original);
  const [staged, setStaged] = useState({ document: original, operations: [] as SchemaEdit[] });
  const [view, setView] = useState('visual');
  const [json, setJson] = useState(() => JSON.stringify(schemaAt(original, address), null, 2));
  const [editing, setEditing] = useState<Parameters<typeof VisualEditor>[0]['editing']>();
  const [error, setError] = useState(''), [importing, setImporting] = useState(false);
  const [copied, setCopied] = useState<string>();
  const change = (apply: SchemaEdit) => {
    try {
      setStaged({ document: apply(staged.document), operations: [...staged.operations, apply] });
      setError(''); return true;
    } catch (error) { setError(error instanceof CommandRejected ? tx(`workflows:rejection.${error.code}`) : tx('workflows:canvas.operation-failed')); return false; }
  };
  const read = () => {
    const result = readSchema(json);
    if (result.error) { setError(tx(result.error === 'root' ? 'workflows:structured.root-object' : result.error === 'number' ? 'workflows:inspector.json-number' : 'workflows:inspector.json-invalid')); return; }
    return result.schema;
  };
  const switchView = (next: unknown) => {
    if (typeof next !== 'string' || next === view) return;
    if (editing) { setError(tx('workflows:structured.finish-field')); return; }
    if (view === 'json') { const schema = read(); if (!schema) return; if (!sameDocument(schema, schemaAt(staged.document, address))) change(replaceSchema(address, schema)); }
    else setJson(JSON.stringify(schemaAt(staged.document, address), null, 2));
    setError(''); setView(next);
  };
  const save = async () => {
    if (editing) { setError(tx('workflows:structured.finish-field')); return; }
    if (!sameDocument(fieldValue(original, address)?.value, fieldValue(base, address)?.value)) { setError(tx('workflows:inspector.field-changed')); return; }
    const schema = view === 'json' ? read() : schemaAt(staged.document, address);
    if (!schema) return;
    const operations = view === 'json' && !sameDocument(schema, schemaAt(staged.document, address)) ? [...staged.operations, replaceSchema(address, schema)] : staged.operations;
    const candidate = operations.reduce((next, apply) => apply(next), original);
    const context = owner.editor.getSnapshot().context;
    input.current = { port: context.port, target: context.target, generation: context.generation, document: candidate };
    setSaving(true); setError('');
    try {
      const report = await validation.current.request(input.current, () => input.current);
      if (!report) return;
      const output = report.analysis.outputs.find(output => output.producer.node === address.node && sameBlockPath(output.producer.block, address.block));
      if (!output?.schema) { setError(output ? tx('workflows:structured.invalid-schema') : report.analysis.diagnostic || tx('workflows:structured.validation-failed')); return; }
      const current = owner.editor.getSnapshot().context.document;
      if (!current || !sameDocument(fieldValue(current, address)?.value, fieldValue(base, address)?.value)) { setError(tx('workflows:inspector.field-changed')); return; }
      edit(document => operations.reduce((next, apply) => apply(next), document));
      onClose();
    } catch { setError(tx('workflows:structured.validation-failed')); }
    finally { setSaving(false); }
  };
  return <Modal open headless title={tx('workflows:structured.title')} onClose={onClose} className={css.dialog}>
    <div className={css.header}><h2>{tx('workflows:structured.title')}</h2><button type="button" className={css.iconButton} aria-label={tx('workflows:dialog.close')} onClick={onClose}><IconCloseOutline16 /></button></div>
    <Tabs.Root className={css.tabs} value={view} onValueChange={switchView} inert={saving}>
      <div className={css.toolbar}>
        <Tabs.List className={css.tabList} aria-label={tx('workflows:structured.title')}>
          <Tabs.Tab className={css.tab} value="visual"><IconListPenOutline16 />{tx('workflows:structured.visual')}</Tabs.Tab>
          <Tabs.Tab className={css.tab} value="json"><IconBraces />{/* i18n-raw: JSON Schema format name */ 'JSON Schema'}</Tabs.Tab>
        </Tabs.List>
        <button type="button" className={css.importButton} onClick={() => { if (editing) setError(tx('workflows:structured.finish-field')); else setImporting(true); }}>{tx('workflows:structured.import-json')}</button>
      </div>
      <Tabs.Panel className={css.workspace} value="visual"><VisualEditor document={staged.document} address={address} root={root} change={change} editing={editing} setEditing={value => { setEditing(value); setError(''); }} /></Tabs.Panel>
      <Tabs.Panel className={css.workspace} value="json"><JsonCodeEditor panel label="JSON" value={json} onChange={value => { setJson(value); setError(''); }} allowEmpty={false}
        headerActions={<button type="button" aria-label={tx(copied === json ? 'workflows:structured.copied' : 'workflows:structured.copy')} onClick={async () => {
          try { await navigator.clipboard.writeText(json); setCopied(json); } catch { setError(tx('workflows:structured.copy-failed')); }
        }}>{copied === json ? <IconCheckOutline16 /> : <IconCopyOutline16 />}</button>} /></Tabs.Panel>
    </Tabs.Root>
    {error && <p className={css.error} role="alert">{error}</p>}
    <div className={css.footer}>
      <Button variant="outline" size="sm" disabled={saving} onClick={() => { setEditing(undefined); change(replaceSchema(address, emptySchema())); setJson(JSON.stringify(emptySchema(), null, 2)); }}>{tx('workflows:structured.clear')}</Button>
      <span className={css.separator} /><Button variant="outline" size="sm" onClick={onClose}>{tx('workflows:dialog.cancel')}</Button>
      <Button variant="primary" size="sm" disabled={saving} onClick={save}>{tx(saving ? 'workflows:structured.validating' : 'workflows:structured.save')}</Button>
    </div>
    {importing && <JsonImporter onClose={() => setImporting(false)} onImport={schema => {
      change(replaceSchema(address, schema)); setJson(JSON.stringify(schema, null, 2)); setImporting(false);
    }} />}
  </Modal>;
}

function JsonImporter({ onClose, onImport }: { onClose: () => void; onImport: (schema: ReturnType<typeof emptySchema>) => void }) {
  const tx = useTranslation(), [json, setJson] = useState(''), [error, setError] = useState('');
  const submit = () => {
    const result = readJson(json);
    if (!result.ok) { setError(tx(result.reason === 'number' ? 'workflows:inspector.json-number' : 'workflows:inspector.json-invalid')); return; }
    if (!schemaObject(result.value)) { setError(tx('workflows:structured.example-object')); return; }
    try { onImport(schemaFromExample(result.value)); }
    catch { setError(tx('workflows:structured.example-shape')); }
  };
  return <Modal open title={tx('workflows:structured.import-json')} closeLabel={tx('workflows:dialog.close')} onClose={onClose} className={css.importDialog}
    footer={<><Button variant="outline" size="sm" onClick={onClose}>{tx('workflows:dialog.cancel')}</Button><Button variant="primary" size="sm" onClick={submit}>{tx('workflows:structured.import')}</Button></>}>
    <p className={css.importHint}>{tx('workflows:structured.import-hint')}</p>
    <JsonCodeEditor label="JSON" value={json} onChange={value => { setJson(value); setError(''); }} allowEmpty={false} error={error} placeholder={tx('workflows:structured.example-placeholder')} minHeight={220} />
  </Modal>;
}
