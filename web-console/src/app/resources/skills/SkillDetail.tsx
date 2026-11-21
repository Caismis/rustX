import { useEffect, useId, useRef, useState, type ChangeEvent } from 'react';
import type { SkillDisplay, SkillDisplayWrite, SkillIconMediaType, SkillIconUpload, SourceScope, SourceTarget } from '../../../../../protocol/app-server/v80';
import type { AppServerClient } from '../../../client/app-server';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { IconTrashOutline16 } from '../../../presentation/primitives/icons';
import { MarkdownText } from '../../../presentation/markdown/MarkdownText';
import { DocumentLinesSkeleton } from '../../../presentation/primitives/Skeleton';
import { SettingsBackButton, SettingsPageShell, SettingsGroup, SettingsRow } from '../../../presentation/settings/SettingsPageShell';
import settings from '../../../presentation/settings/SettingsContent.module.css';
import { splitFrontmatter } from '../../components/workbench-documents/frontmatter';
import { PageTransactionFooter, usePageTransaction } from '../../settings/forms/bridge';
import { useNavigationGuard } from '../../settings/forms/guard';
import { IconPicker } from '../../workflows/ui/WorkflowIcon';
import { SkillIcon, skillAssetUrl } from './SkillIcon';
import css from './SkillDetail.module.css';

export type SkillDisplayWriter = (authored: SkillDisplayWrite, upload?: SkillIconUpload) => Promise<void>;

const MAX_ICON_BYTES = 32 * 1024;
const IMAGE_TYPES: Readonly<Record<string, SkillIconMediaType>> = {
  png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', webp: 'image/webp', svg: 'image/svg+xml',
};

/** In-page skill detail: native instructions are read-only, while the display
 * metadata (name, short description, icon) stages into the page transaction
 * and commits through the shared page footer. */
export function SkillDetail({ client, target, name, display, owner, write, onDelete, close, bodyDisabled = false }: {
  client: AppServerClient; target: SourceTarget; name: string;
  display?: SkillDisplay | null; owner: SourceScope;
  write?: (authored: SkillDisplayWrite, upload?: SkillIconUpload) => Promise<void>;
  onDelete?: () => void; close: () => void; bodyDisabled?: boolean;
}) {
  const tx = useTranslation();
  const guard = useNavigationGuard();
  const page = usePageTransaction();
  const [content, setContent] = useState<{ text: string; path: string; description: string; owner: SourceScope }>();
  const [error, setError] = useState('');
  useEffect(() => {
    setError(''); setContent(undefined);
    let alive = true;
    const generation = client.getSnapshot().generation;
    const current = () => alive && client.getSnapshot().generation === generation;
    void (async () => {
      let offset = '0', revision: string | undefined, text = '';
      const decoder = new TextDecoder();
      do {
        const { content } = await client.request({ method: 'skills/read', params: { target, name, kind: 'instructions', offset, max_bytes: 65536, expected_revision: revision } }, 'skill_content', undefined, current);
        if (!current()) return;
        const bytes = Uint8Array.from(atob(content.data_base64), char => char.charCodeAt(0));
        text += decoder.decode(bytes, { stream: content.next_offset != null });
        revision = content.revision;
        if (content.next_offset == null) {
          const { location, description } = content.skill.catalog;
          setContent({ text: splitFrontmatter(text)?.body ?? text, path: location, description, owner: content.skill.provenance.source });
          return;
        }
        offset = content.next_offset;
      } while (current());
    })().catch(error => { if (current()) setError(String(error)); });
    return () => { alive = false; };
  }, [client, target, name]);

  const authored = display?.authored;
  const [draft, setDraft] = useState<SkillDisplayWrite>(authored ?? {});
  const [selectedImage, setSelectedImage] = useState<{ name: string; upload: SkillIconUpload }>();
  const [reading, setReading] = useState(false);
  const [failure, setFailure] = useState('');
  const fileInput = useRef<HTMLInputElement>(null);
  // A committed save can echo server-normalized authored values (e.g. a
  // generated asset icon path): re-seed the staged draft when the native
  // display revision advances because of our own write, or while no user
  // edit is staged. An external advance over a staged draft is a real
  // divergence the CAS layer must reject, not something to silently absorb.
  const lastRevision = useRef(display?.revision);
  const userEdited = useRef(false);
  const ownWrite = useRef(false);
  useEffect(() => {
    if (lastRevision.current === display?.revision) return;
    lastRevision.current = display?.revision;
    if (ownWrite.current || !userEdited.current) {
      setDraft(display?.authored ?? {});
      setSelectedImage(undefined);
      setFailure('');
      userEdited.current = false;
    }
    ownWrite.current = false;
  }, [display]);
  const iconSectionId = useId();

  const update = (key: keyof SkillDisplayWrite, value: string) => {
    userEdited.current = true;
    setDraft(current => ({ ...current, [key]: value }));
    if (key === 'icon_small' || key === 'icon_large') { setSelectedImage(undefined); setFailure(''); }
  };
  const clean = Object.fromEntries(Object.entries(draft).flatMap(([key, value]) => {
    const text = value?.trim();
    return text ? [[key, text]] : [];
  }));
  const changed = !!selectedImage || (Object.keys({ ...authored, ...clean }) as (keyof SkillDisplayWrite)[])
    .some(key => authored?.[key] !== clean[key]);
  const pathsChanged = clean.icon_small !== authored?.icon_small || clean.icon_large !== authored?.icon_large;
  const assetPath = clean.icon_small ?? clean.icon_large;
  const preview = selectedImage ? { kind: 'asset' as const, ...selectedImage.upload }
    : !pathsChanged && display?.icon?.kind === 'asset' ? display.icon
      : clean.icon ? { kind: 'iconify' as const, name: clean.icon } : undefined;
  const iconLabel = selectedImage ? selectedImage.name
    : assetPath ? tx('resources:skills.asset-icon') : clean.icon ?? tx('resources:skills.default-icon');
  const setIcon = (iconName?: string) => {
    userEdited.current = true;
    setDraft(current => ({ ...current, icon: iconName, icon_small: undefined, icon_large: undefined }));
    setSelectedImage(undefined); setFailure('');
  };
  const chooseImage = async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.currentTarget.files?.[0];
    event.currentTarget.value = '';
    if (!file) return;
    const extension = file.name.split('.').at(-1)?.toLowerCase() ?? '';
    const mediaType = Object.hasOwn(IMAGE_TYPES, extension) ? IMAGE_TYPES[extension] : undefined;
    if (!mediaType || !file.size || file.size > MAX_ICON_BYTES) { setFailure(tx('resources:skills.upload-invalid')); return; }
    setReading(true); setFailure('');
    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      const upload = { media_type: mediaType, data_base64: btoa(String.fromCharCode(...bytes)) };
      const image = new Image();
      image.src = skillAssetUrl(upload);
      await image.decode();
      userEdited.current = true;
      setSelectedImage({ name: file.name, upload });
    } catch { setFailure(tx('resources:skills.upload-invalid')); }
    finally { setReading(false); }
  };

  // Display fields stage as a page-local draft: the footer commits them
  // through the same save gesture as every other unit on the page.
  useEffect(() => {
    const editable = !!write && !!display?.revision;
    page?.setLocal(editable && changed,
      editable ? () => {
        ownWrite.current = true;
        void Promise.resolve(write(clean, selectedImage?.upload))
          // A committed upload must release the preview intent — keeping it
          // would hold `changed` true and the page would stay dirty forever.
          .then(() => setSelectedImage(undefined))
          .catch(error => { ownWrite.current = false; setFailure(String(error)); });
      } : undefined,
      editable ? () => { setDraft(authored ?? {}); setSelectedImage(undefined); setFailure(''); userEdited.current = false; } : undefined);
    // `draft`/`selectedImage` must be deps: the save closure commits exactly
    // what they captured — a boolean `changed` alone would keep a stale clean.
  }, [page, changed, draft, selectedImage, write, display?.revision]);

  const title = display?.authored.display_name ?? name;
  const editable = !!write && !!display?.revision;
  return <SettingsPageShell title={title} className={`${settings.settings} ${settings.page} ${css.detail}`}
    bodyDisabled={bodyDisabled}
    breadcrumb={<SettingsBackButton label={tx('resources:skills.title')} current={title} onClick={() => guard.guard(close)} />}
    footer={editable ? <PageTransactionFooter /> : null}>
    <div className={css.detailTop}>
      <SkillIcon icon={preview ?? display?.icon} size={48} />
      <div className={css.detailMeta}>
        <span className={css.scopeTag}>{tx(owner === 'user' ? 'resources:scope.global' : 'resources:scope.workspace')}</span>
        {content?.path && <code>{content.path}</code>}
      </div>
      {onDelete && <Button variant="outline" size="sm" icon={<IconTrashOutline16 />} onClick={onDelete}>{tx('resources:skills.delete')}</Button>}
    </div>
    {editable && <SettingsGroup title={tx('resources:skills.edit-display')}>
      <SettingsRow label={tx('resources:skills.display-name')}>
        <input value={draft.display_name ?? ''} maxLength={128} aria-label={tx('resources:skills.display-name')}
          onChange={event => update('display_name', event.target.value)} />
      </SettingsRow>
      <SettingsRow label={tx('resources:skills.short-description')} vertical>
        <textarea value={draft.short_description ?? ''} rows={2} maxLength={1024} aria-label={tx('resources:skills.short-description')}
          onChange={event => update('short_description', event.target.value)} />
      </SettingsRow>
      <SettingsRow label={tx('resources:skills.icon')} vertical description={iconLabel}>
        <div className={css.iconActions}>
          <input ref={fileInput} type="file" hidden accept=".png,.jpg,.jpeg,.webp,.svg"
            aria-label={tx('resources:skills.upload-image')} onChange={event => void chooseImage(event)} />
          <Button variant="outline" size="sm" disabled={reading} onClick={() => fileInput.current?.click()}>
            {tx('resources:skills.upload-image')}
          </Button>
          <IconPicker value={clean.icon ?? ''} label={tx('resources:skills.choose-iconify')}
            onChange={setIcon} disabled={reading} />
          {(selectedImage || assetPath || clean.icon) && <Button variant="ghost" size="sm" onClick={() => setIcon()}>
            {tx('resources:skills.clear-icon')}
          </Button>}
        </div>
      </SettingsRow>
      <SettingsRow label={tx('resources:skills.asset-paths')} vertical>
        <div className={css.pathFields} id={iconSectionId}>
          <label>{tx('resources:skills.small-icon-path')}
            <input value={draft.icon_small ?? ''} spellCheck={false} maxLength={1024}
              onChange={event => update('icon_small', event.target.value)} /></label>
          <label>{tx('resources:skills.large-icon-path')}
            <input value={draft.icon_large ?? ''} spellCheck={false} maxLength={1024}
              onChange={event => update('icon_large', event.target.value)} /></label>
        </div>
      </SettingsRow>
      {failure && <p role="alert" className={css.error}>{failure}</p>}
    </SettingsGroup>}
    <SettingsGroup title={tx('resources:skills.instructions')}>
      <div className={css.instructionsBody}>
        {error ? <p role="alert">{error}</p> : content ? <>
          {!!content.description && <p className={css.description}>{content.description}</p>}
          {content.text.trim() ? <MarkdownText text={content.text} /> : <p>{tx('resources:skills.empty-instructions')}</p>}
        </> : <DocumentLinesSkeleton label={tx('resources:loading')} />}
      </div>
    </SettingsGroup>
  </SettingsPageShell>;
}
