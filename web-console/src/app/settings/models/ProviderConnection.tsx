import { SettingsNotice } from '../SettingsFeedback';
import { useId, useRef, useState } from 'react';
import type { ProviderView, ProviderWrite, SourceScope } from '../../../../../protocol/app-server/v77';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { Menu } from '../../../presentation/primitives/Menu';
import { Switch } from '../../../presentation/primitives/Switch';
import { IconEllipsisOutline16, IconTrashOutline16 } from '../../../presentation/primitives/icons';
import { UnitOutcomeNotice, useUnitEditing } from '../forms/bridge';
import { Choice, ConfirmAction } from '../primitives/aria';
import { IconPicker, ProviderIcon } from './ProviderIcon';
import { providerMutation, providerTemplates } from './ProviderTemplates';
import cards from './ModelsPage.module.css';

export function ProviderConnection({ id, scope, revision, authored, inherited, hasModels, onCancel }: {
  id: string; scope: SourceScope; revision: string; authored?: ProviderView; inherited?: ProviderView;
  hasModels: boolean; onCancel: () => void;
}) {
  const tx = useTranslation();
  const unit = useUnitEditing<ProviderWrite>({
    authored: authored && { hidden: authored.hidden, icon: authored.icon, protocol: authored.protocol, base_url: authored.base_url, credential: { kind: 'retain' } },
    blank: {
      icon: inherited?.icon, hidden: inherited?.hidden,
      protocol: inherited?.protocol ?? 'openai_chat_completions',
      base_url: inherited?.base_url ?? '',
      credential: { kind: 'literal', value: '' },
    },
    revision,
    mutation: value => providerMutation(id, value),
  });
  const [menu, setMenu] = useState(false);
  const [iconPicker, setIconPicker] = useState(false);
  const [showKey, setShowKey] = useState(false);
  const [customizing, setCustomizing] = useState(false);
  const [attempted, setAttempted] = useState(0);
  const form = useRef<HTMLFormElement>(null);
  const keyId = useId();
  const title = tx('settings:models-page.provider-value', { p0: id });
  const value = unit.displayed;
  const inspecting = !!inherited && !authored && !customizing && !unit.draft;
  const storedCredential = authored?.credential ?? (inspecting ? inherited.credential : undefined);
  const credential = (value.credential.kind === 'retain' || inspecting) && storedCredential?.type === 'environment'
    ? { kind: 'environment' as const, variable: storedCredential.variable } : value.credential;
  const key = credential.kind === 'literal' ? credential.value : '';
  const getKey = providerTemplates.find(item => item.url === value.base_url.replace(/\/$/, ''))?.keyUrl;
  const locked = unit.busy || unit.awaitingObservation;
  const creating = !authored && !inherited;
  const restores = scope === 'workspace' && !!inherited;
  const canRemove = !!authored && (restores || !hasModels);
  const endpointError = (() => {
    try {
      const url = new URL(value.base_url);
      return !['http:', 'https:'].includes(url.protocol) || !url.hostname || !!(url.username || url.password || url.search || url.hash);
    } catch { return true; }
  })();
  const credentialError = credential.kind === 'literal' ? !key.trim() || key.startsWith('$')
    : credential.kind === 'environment' && !/^[a-zA-Z_][a-zA-Z0-9_]*$/.test(credential.variable);
  const edit = (next: ProviderWrite) => { setAttempted(0); unit.edit(next); };
  const changeCredential = (next: ProviderWrite['credential']) => edit({ ...value, credential: next });
  const discard = () => {
    unit.discard();
    setAttempted(0);
    setShowKey(false);
    setCustomizing(false);
    if (creating && !hasModels) onCancel();
  };

  return <form ref={form} tabIndex={-1} aria-label={title} className={cards.connection} noValidate
    onSubmit={event => {
      event.preventDefault();
      setAttempted(attempt => attempt + 1);
      if (!endpointError && !credentialError && unit.draft && unit.admitted && !locked) {
        setShowKey(false);
        unit.submit();
      }
    }}>
    <div className={cards.detailHeader}>
      <h3 title={id} aria-label={title}><ProviderIcon icon={value.icon} size={18}/><span>{id}</span></h3>
      {!creating && <Switch className={cards.switch} checked={!value.hidden} label={tx('settings:models.show-provider', { p0: id })}
        disabled={!authored || !unit.admitted || locked || unit.draft} onChange={checked => unit.apply({ ...value, hidden: !checked })}/>}
      <ConfirmAction
        label={tx('settings:bridge.remove-value', { p0: title })}
        title={restores ? tx('settings:bridge.use-the-global-default-for-value', { p0: title }) : tx('settings:bridge.remove-value-from-value-configuration', { p0: title, p1: tx(scope === 'workspace' ? 'settings:extension-detail.workspace' : 'settings:extension-detail.user') })}
        confirm={restores ? tx('settings:bridge.use-global-default-value', { p0: title }) : tx('settings:bridge.remove-value', { p0: title })}
        description={tx(restores ? 'settings:forms.restore-help' : 'settings:forms.delete-help')}
        tone={restores ? 'restore' : 'destructive'} settle={form} onConfirm={() => unit.submit(true)}
        renderTrigger={(open, ref) => <Menu open={menu} align="end" compact autoFocus onClose={() => setMenu(false)}
          anchor={<button type="button" ref={ref} className={cards.iconButton} aria-label={tx('settings:models.provider-actions')} aria-haspopup="menu" aria-expanded={menu}
            disabled={locked} onClick={() => setMenu(!menu)}><IconEllipsisOutline16/></button>}
          items={inspecting
            ? [{ id: 'customize', label: tx('settings:forms.customize') }]
            : [
              { id: 'icon', label: tx('settings:models.choose-icon') },
              { id: 'credential', label: tx(credential.kind === 'environment' ? 'settings:models.use-key' : 'settings:models.use-environment') },
              ...authored ? [{ id: 'delete', label: tx(restores ? 'settings:models.restore' : canRemove ? 'settings:copy.delete' : 'settings:models.remove-models-first'),
                icon: <IconTrashOutline16/>, danger: !restores, disabled: !canRemove || !unit.admitted }] : [],
            ]}
          onSelect={action => {
            setMenu(false);
            if (action === 'icon') setIconPicker(true);
            if (action === 'credential') changeCredential(credential.kind === 'environment' ? { kind: 'literal', value: '' } : { kind: 'environment', variable: '' });
            if (action === 'customize') setCustomizing(true);
            if (action === 'delete') open();
          }}/>}
      />
    </div>
    {iconPicker && <IconPicker value={value.icon} onChange={icon => edit({ ...value, icon })} onClose={() => setIconPicker(false)}
      finalFocus={() => form.current?.querySelector<HTMLButtonElement>('button[aria-haspopup="menu"]') ?? false}/>}
    {inspecting && <p className={cards.inherited}>{tx('settings:forms.using-global')}</p>}
    <fieldset disabled={locked || inspecting} className={cards.connectionFields}>
      <label>{tx('settings:models-page.endpoint')}
        <input type="url" required value={value.base_url} aria-invalid={attempted > 0 && endpointError}
          placeholder={/* i18n-raw: example URL */ 'https://api.example.com/v1'}
          onChange={event => edit({ ...value, base_url: event.target.value })}/>
      </label>
      <Choice label={tx('settings:models-page.protocol')} value={value.protocol} disabled={locked || inspecting} options={[
        ['openai_chat_completions', tx('settings:models.api-chat')],
        ['openai_responses', tx('settings:models.api-responses')],
        ['anthropic_messages', tx('settings:models.api-anthropic')],
      ]} onChange={protocol => edit({ ...value, protocol })}/>
      {credential.kind === 'environment' ? <label>{tx('settings:models-page.environment-variable')}
        <input required value={credential.variable} aria-invalid={attempted > 0 && credentialError}
          onChange={event => changeCredential({ kind: 'environment', variable: event.target.value })}/>
      </label> : <div className={cards.secret}>
        <div className={cards.keyHeading}>
          <label htmlFor={keyId}>{tx('settings:models-page.new-literal-credential')}</label>
          {getKey && !authored && <a href={getKey} target="_blank" rel="noreferrer">{tx('settings:models.get-key')}</a>}
        </div>
        <input id={keyId} type={showKey ? 'text' : 'password'} autoComplete="new-password" spellCheck={false}
          value={key} aria-invalid={attempted > 0 && credentialError}
          placeholder={credential.kind === 'retain' || inspecting ? tx('settings:models.saved-key-placeholder') : tx('settings:models.enter-key')}
          onChange={event => changeCredential(event.target.value === '' && authored?.credential.type === 'literal'
            ? { kind: 'retain' } : { kind: 'literal', value: event.target.value })}/>
        <Button size="sm" className={cards.iconButton} disabled={!key} aria-pressed={showKey}
          aria-label={tx(showKey ? 'settings:models.hide-key' : 'settings:models.show-key')} onClick={() => setShowKey(!showKey)}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" aria-hidden="true">
            <path d="M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12Z"/><circle cx="12" cy="12" r="3"/>{showKey && <path d="m3 3 18 18"/>}
          </svg>
        </Button>
      </div>}
    </fieldset>
    <SettingsNotice occurrence={attempted} text={attempted ? endpointError ? tx('settings:models.invalid-endpoint') : credentialError ? tx('settings:models.invalid-key') : undefined : undefined}/>
    <UnitOutcomeNotice title={title} outcome={unit.outcome}/>
    <div className={cards.connectionActions} data-clean={(!creating || hasModels) && !unit.intent && !customizing && !locked}>
      <Button size="sm" onClick={discard} disabled={locked}>{tx(creating && !hasModels ? 'settings:aria.cancel' : 'settings:bridge.discard-draft')}</Button>
      {unit.reviewNeeded && <Button size="sm" onClick={unit.review}>{tx('settings:bridge.use-reviewed-revision')}</Button>}
      <Button size="sm" variant="primary" type="submit" aria-label={`${tx('settings:bridge.save')} ${title}`}
        aria-busy={locked} disabled={!unit.draft || !unit.admitted || locked}>{tx(locked ? 'settings:catalog.saving' : 'settings:bridge.save')}</Button>
    </div>
  </form>;
}
