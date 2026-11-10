import { useId, useRef, useState, type ChangeEvent } from 'react';
import type { SkillDisplay, SkillDisplayWrite } from '../../../../../protocol/app-server/v77';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { Modal } from '../../../presentation/primitives/Modal';
import settings from '../../../presentation/settings/SettingsContent.module.css';
import { IconPicker } from '../../workflows/ui/WorkflowIcon';
import { SkillIcon } from './SkillIcon';
import css from './SkillDisplayDialog.module.css';

export type SkillDisplayWriter = (authored: SkillDisplayWrite) => Promise<void>;

export function SkillDisplayDialog({ name, display, save, close }: {
  name: string; display: SkillDisplay; save: SkillDisplayWriter; close: () => void;
}) {
  const tx = useTranslation();
  const formId = useId();
  const { authored, icon } = display;
  const [draft, setDraft] = useState(authored);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState('');
  const submitting = useRef(false);
  const field = (key: keyof SkillDisplayWrite) => ({
    value: draft[key] ?? '',
    onChange: (event: ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => setDraft(current => ({ ...current, [key]: event.target.value })),
  });
  const clean = Object.fromEntries(Object.entries(draft).flatMap(([key, value]) => {
    const text = value?.trim();
    return text ? [[key, text]] : [];
  }));
  const changed = (Object.keys({ ...authored, ...clean }) as (keyof SkillDisplayWrite)[]).some(key => authored[key] !== clean[key]);
  const asset = icon?.kind === 'asset' && draft.icon_small === authored.icon_small && draft.icon_large === authored.icon_large
    ? icon : undefined;
  const preview = asset ?? (draft.icon ? { kind: 'iconify' as const, name: draft.icon } : undefined);
  const submit = async () => {
    if (submitting.current || !changed) return;
    submitting.current = true;
    setBusy(true);
    setFailure('');
    try {
      await save(clean);
      close();
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  };
  const dismiss = () => { if (!submitting.current) close(); };
  return <Modal open onClose={dismiss} title={tx('resources:skills.edit-display')} closeLabel={tx('resources:close')}
    description={tx('resources:skills.edit-display-help', { p0: name })}
    footer={<>
      <Button variant="outline" disabled={busy} onClick={dismiss}>{tx('resources:cancel')}</Button>
      <Button variant="primary" type="submit" form={formId} disabled={!changed || busy}>{tx('settings:bridge.save')}</Button>
    </>}>
    <form id={formId} className={`${settings.settings} ${css.form}`} onSubmit={event => { event.preventDefault(); void submit(); }}>
      <fieldset disabled={busy}>
        <label>{tx('resources:skills.display-name')}<input {...field('display_name')} maxLength={128} /></label>
        <label>{tx('resources:skills.short-description')}<textarea {...field('short_description')} rows={2} maxLength={1024} /></label>
        <div className={css.icon}>
          <IconPicker value={draft.icon ?? ''} preview={<SkillIcon icon={preview} size={22} />}
            onChange={icon => setDraft(current => ({ ...current, icon, icon_small: undefined, icon_large: undefined }))} disabled={busy} />
          <span>{asset ? tx('resources:skills.asset-icon') : draft.icon ?? tx('resources:skills.default-icon')}</span>
          <Button variant="ghost" size="sm" disabled={busy || (!draft.icon && !draft.icon_small && !draft.icon_large)}
            onClick={() => setDraft(current => ({ ...current, icon: undefined, icon_small: undefined, icon_large: undefined }))}>
            {tx('resources:skills.clear-icon')}
          </Button>
        </div>
        <label>{tx('resources:skills.small-icon-path')}<input {...field('icon_small')} spellCheck={false} maxLength={1024} /></label>
        <label>{tx('resources:skills.large-icon-path')}<input {...field('icon_large')} spellCheck={false} maxLength={1024} /></label>
        <small>{tx('resources:skills.icon-path-help')}</small>
      </fieldset>
      {failure && <p role="alert" className={css.error}>{failure}</p>}
    </form>
  </Modal>;
}
