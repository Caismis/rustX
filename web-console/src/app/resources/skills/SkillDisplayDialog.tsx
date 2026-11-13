import { useId, useRef, useState, type ChangeEvent } from 'react';
import type { SkillDisplay, SkillDisplayWrite, SkillIconMediaType, SkillIconUpload } from '../../../../../protocol/app-server/v78';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { Modal } from '../../../presentation/primitives/Modal';
import settings from '../../../presentation/settings/SettingsContent.module.css';
import { IconPicker } from '../../workflows/ui/WorkflowIcon';
import { SkillIcon, skillAssetUrl } from './SkillIcon';
import css from './SkillDisplayDialog.module.css';

export type SkillDisplayWriter = (authored: SkillDisplayWrite, upload?: SkillIconUpload) => Promise<void>;

const MAX_ICON_BYTES = 32 * 1024;
const IMAGE_TYPES: Readonly<Record<string, SkillIconMediaType>> = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  webp: 'image/webp',
  svg: 'image/svg+xml',
};

export function SkillDisplayDialog({ name, display, save, close }: {
  name: string; display: SkillDisplay; save: SkillDisplayWriter; close: () => void;
}) {
  const tx = useTranslation();
  const formId = useId();
  const { authored, icon } = display;
  const [draft, setDraft] = useState(authored);
  const [selectedImage, setSelectedImage] = useState<{ name: string; upload: SkillIconUpload }>();
  const [reading, setReading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState('');
  const submitting = useRef(false);
  const fileInput = useRef<HTMLInputElement>(null);

  const update = (key: keyof SkillDisplayWrite, value: string) => {
    setDraft(current => ({ ...current, [key]: value }));
    if (key === 'icon_small' || key === 'icon_large') {
      setSelectedImage(undefined);
      setFailure('');
    }
  };
  const field = (key: keyof SkillDisplayWrite) => ({
    value: draft[key] ?? '',
    onChange: (event: ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => update(key, event.target.value),
  });
  const clean = Object.fromEntries(Object.entries(draft).flatMap(([key, value]) => {
    const text = value?.trim();
    return text ? [[key, text]] : [];
  }));
  const changed = !!selectedImage || (Object.keys({ ...authored, ...clean }) as (keyof SkillDisplayWrite)[])
    .some(key => authored[key] !== clean[key]);

  const pathsChanged = clean.icon_small !== authored.icon_small || clean.icon_large !== authored.icon_large;
  const assetPath = clean.icon_small ?? clean.icon_large;
  const preview = selectedImage ? { kind: 'asset' as const, ...selectedImage.upload }
    : !pathsChanged && icon?.kind === 'asset' ? icon
      : clean.icon ? { kind: 'iconify' as const, name: clean.icon } : undefined;
  const iconLabel = selectedImage ? selectedImage.name
    : assetPath ? tx('resources:skills.asset-icon') : clean.icon ?? tx('resources:skills.default-icon');
  const iconDescription = selectedImage ? tx('resources:skills.upload-pending') : assetPath ?? tx('resources:skills.upload-help');

  const setIcon = (name?: string) => {
    setDraft(current => ({ ...current, icon: name, icon_small: undefined, icon_large: undefined }));
    setSelectedImage(undefined);
    setFailure('');
  };
  const chooseImage = async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.currentTarget.files?.[0];
    event.currentTarget.value = '';
    if (!file) return;

    const extension = file.name.split('.').at(-1)?.toLowerCase() ?? '';
    const mediaType = Object.hasOwn(IMAGE_TYPES, extension) ? IMAGE_TYPES[extension] : undefined;
    if (!mediaType || !file.size || file.size > MAX_ICON_BYTES) {
      setFailure(tx('resources:skills.upload-invalid'));
      return;
    }
    setReading(true);
    setFailure('');
    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      const upload = { media_type: mediaType, data_base64: btoa(String.fromCharCode(...bytes)) };
      const image = new Image();
      image.src = skillAssetUrl(upload);
      await image.decode();
      setSelectedImage({ name: file.name, upload });
    } catch {
      setFailure(tx('resources:skills.upload-invalid'));
    } finally {
      setReading(false);
    }
  };
  const submit = async () => {
    if (submitting.current || reading || !changed) return;
    submitting.current = true;
    setBusy(true);
    setFailure('');
    try {
      await save(clean, selectedImage?.upload);
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
    className={css.dialog} description={tx('resources:skills.edit-display-help', { p0: name })}
    footer={<>
      <Button variant="outline" disabled={busy} onClick={dismiss}>{tx('resources:cancel')}</Button>
      <Button variant="primary" type="submit" form={formId} disabled={!changed || busy || reading}>{tx('settings:bridge.save')}</Button>
    </>}>
    <form id={formId} className={`${settings.settings} ${css.form}`} aria-busy={busy || reading}
      onSubmit={event => { event.preventDefault(); void submit(); }}>
      <fieldset disabled={busy || reading}>
        <label>{tx('resources:skills.display-name')}<input {...field('display_name')} maxLength={128} /></label>
        <label>{tx('resources:skills.short-description')}<textarea {...field('short_description')} rows={2} maxLength={1024} /></label>
        <section className={css.iconSection} aria-labelledby={`${formId}-icon`}>
          <h3 id={`${formId}-icon`}>{tx('resources:skills.icon')}</h3>
          <div className={css.preview}>
            <span className={css.iconTile}><SkillIcon icon={preview} size={34} /></span>
            <div className={css.iconInfo} aria-live="polite">
              <strong>{iconLabel}</strong>
              <small>{iconDescription}</small>
            </div>
          </div>
          <div className={css.iconActions}>
            <input ref={fileInput} type="file" hidden accept=".png,.jpg,.jpeg,.webp,.svg"
              aria-label={tx('resources:skills.upload-image')} onChange={event => void chooseImage(event)} />
            <Button variant="outline" size="sm" onClick={() => fileInput.current?.click()}>
              {tx('resources:skills.upload-image')}
            </Button>
            <IconPicker value={clean.icon ?? ''} label={tx('resources:skills.choose-iconify')} className={css.iconPicker}
              onChange={setIcon} disabled={busy || reading} />
            {(selectedImage || assetPath || clean.icon) && <Button variant="ghost" size="sm" onClick={() => setIcon()}>
              {tx('resources:skills.clear-icon')}
            </Button>}
          </div>
          <details className={css.assetPaths}>
            <summary>{tx('resources:skills.asset-paths')}</summary>
            <div className={css.pathFields}>
              <label>{tx('resources:skills.small-icon-path')}<input {...field('icon_small')} spellCheck={false} maxLength={1024} /></label>
              <label>{tx('resources:skills.large-icon-path')}<input {...field('icon_large')} spellCheck={false} maxLength={1024} /></label>
              <small>{tx('resources:skills.icon-path-help')}</small>
            </div>
          </details>
        </section>
      </fieldset>
      {failure && <p role="alert" className={css.error}>{failure}</p>}
    </form>
  </Modal>;
}
