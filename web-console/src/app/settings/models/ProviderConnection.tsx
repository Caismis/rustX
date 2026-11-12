import { SettingsNotice } from '../SettingsFeedback';
import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import type { ProviderView, ProviderWrite, SourceScope } from '../../../../../protocol/app-server/v79';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { Menu } from '../../../presentation/primitives/Menu';
import { Switch } from '../../../presentation/primitives/Switch';
import { IconEditOutline16, IconEllipsisOutline16, IconTrashOutline16 } from '../../../presentation/primitives/icons';
import { UnitOutcomeNotice, useUnitEditing } from '../forms/bridge';
import { useSettingsActor } from '../machines/react';
import { Choice, ConfirmAction } from '../primitives/aria';
import { IconPicker, ProviderIcon } from './ProviderIcon';
import { providerMutation, providerTemplates } from './ProviderTemplates';
import cards from './ModelsPage.module.css';

export function ProviderConnection({ id, scope, revision, authored, inherited, hasModels }: {
  id: string; scope: SourceScope; revision: string; authored?: ProviderView; inherited?: ProviderView;
  hasModels: boolean;
}) {
  const tx = useTranslation();
  const actor = useSettingsActor();
  const restores = scope === 'workspace' && !!inherited;
  const unit = useUnitEditing<ProviderWrite>({
    authored: authored && { display_name: authored.display_name, hidden: authored.hidden, icon: authored.icon, protocol: authored.protocol, base_url: authored.base_url, credential: { kind: 'retain' } },
    blank: {
      display_name: inherited?.display_name, icon: inherited?.icon, hidden: inherited?.hidden,
      protocol: inherited?.protocol ?? 'openai_chat_completions',
      base_url: inherited?.base_url ?? '',
      credential: { kind: 'literal', value: '' },
    },
    // A redacted ProviderView is safe to inspect but cannot author credentials.
    inherited: () => undefined,
    revision,
    draftKey: JSON.stringify(providerMutation(id, null)),
    mutation: value => providerMutation(id, value, restores),
  });
  const identity = unit.identity;
  const autosave = (action: 'idle' | 'blur' | 'now' | 'flush' | 'hold' | 'cancel') => actor.send({ type: 'UNIT.AUTOSAVE', identity, action });
  useEffect(() => () => actor.send({ type: 'UNIT.AUTOSAVE', identity, action: 'flush' }), [actor, identity]);
  const [menu, setMenu] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [invalidName, setInvalidName] = useState(0);
  const [iconPicker, setIconPicker] = useState(false);
  const [showKey, setShowKey] = useState(false);
  const [customizing, setCustomizing] = useState(false);
  if (authored && customizing) setCustomizing(false);
  const [attempted, setAttempted] = useState(0);
  const form = useRef<HTMLFormElement>(null);
  const keyEditor = useRef<HTMLDivElement>(null);
  const nameInput = useRef<HTMLInputElement>(null);
  const nameEditing = useRef({ cancelled: false, composing: false });
  useLayoutEffect(() => {
    if (!renaming) return;
    nameInput.current?.focus();
    nameInput.current?.select();
  }, [renaming]);
  const keyId = useId();
  const value = unit.displayed;
  const inspecting = !!inherited && !authored && !customizing && !unit.draft;
  const name = (inspecting ? inherited.display_name : value.display_name) ?? id;
  const title = tx('settings:models-page.provider-value', { p0: name });
  const storedCredential = authored?.credential ?? (inspecting ? inherited.credential : undefined);
  const credential = (value.credential.kind === 'retain' || inspecting) && storedCredential?.type === 'environment'
    ? { kind: 'environment' as const, variable: storedCredential.variable } : value.credential;
  const key = credential.kind === 'literal' ? credential.value : '';
  const keyVisible = showKey && !!key;
  const getKey = providerTemplates.find(item => item.url === value.base_url.replace(/\/$/, ''))?.keyUrl;
  const locked = unit.busy || unit.awaitingObservation;
  const creating = !authored && !inherited;
  const canDiscardCreation = creating && unit.intent && !hasModels;
  const { endpointError, credentialError } = connectionErrors(value);
  const canRetry = unit.draft && unit.admitted && !unit.reviewNeeded && !endpointError && !credentialError
    && (unit.outcome.kind === 'rejected' || unit.outcome.kind === 'uncertain');
  const edit = (next: ProviderWrite, trigger?: 'idle' | 'blur' | 'now') => {
    setAttempted(0);
    unit.edit(next);
    const errors = connectionErrors(next);
    if (errors.endpointError || errors.credentialError) autosave('cancel');
    else autosave(trigger ?? 'idle');
  };
  const changeCredential = (next: ProviderWrite['credential']) =>
    // Acknowledgement redacts the new literal. Wait until the user finishes
    // editing it so a pause while typing cannot replace the remaining prefix.
    edit({ ...value, credential: next }, next.kind === 'literal' ? 'blur' : undefined);
  const flush = () => {
    if (!unit.draft) return;
    if (endpointError || credentialError) setAttempted(attempt => attempt + 1);
    else autosave('flush');
  };
  const actionsTrigger = () => form.current?.querySelector<HTMLButtonElement>('button[aria-haspopup="menu"]');
  const finishRename = (input: string) => {
    setRenaming(false);
    const next = input.trim();
    if (nameEditing.current.cancelled || !next || next === name) {
      autosave('flush');
      return;
    }
    if ([...next].length > 100 || /[\p{Cc}\p{Zl}\p{Zp}]/u.test(next)) {
      setInvalidName(occurrence => occurrence + 1);
      autosave('flush');
      return;
    }
    edit({ ...value, display_name: next === id ? undefined : next }, 'now');
  };

  return <form ref={form} tabIndex={-1} aria-label={title} aria-busy={locked} className={cards.connection} noValidate
    onSubmit={event => { event.preventDefault(); flush(); }}
    onKeyDown={event => {
      if (event.key !== 'Enter' || !(event.target instanceof HTMLInputElement) || event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return;
      event.preventDefault();
      flush();
    }}
    onBlur={event => {
      const leavingKey = keyEditor.current?.contains(event.target) && !keyEditor.current.contains(event.relatedTarget);
      const leavingForm = event.target instanceof HTMLInputElement && event.target !== nameInput.current && !event.currentTarget.contains(event.relatedTarget);
      if (leavingKey || leavingForm) {
        if (endpointError || credentialError) return;
        flush();
      }
    }}>
    <div className={cards.detailHeader}>
      <h3 title={name} aria-label={title}>
        <ProviderIcon icon={value.icon} size={18}/>
        {renaming ? <input ref={nameInput} className={cards.providerRename} type="text" defaultValue={name}
          aria-label={tx('settings:models.provider-display-name')}
          onCompositionStart={() => { nameEditing.current.composing = true; }}
          onCompositionEnd={() => { nameEditing.current.composing = false; }}
          onBlur={event => finishRename(event.currentTarget.value)}
          onKeyDown={event => {
            if (event.key !== 'Enter' && event.key !== 'Escape') return;
            event.stopPropagation();
            if (nameEditing.current.composing || event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return;
            event.preventDefault();
            nameEditing.current.cancelled = event.key === 'Escape';
            actionsTrigger()?.focus();
          }}/>
          : <span>{name}</span>}
      </h3>
      {!creating && <Switch checked={!value.hidden} label={tx('settings:models.show-provider', { p0: name })}
        disabled={!authored || locked || endpointError || credentialError} onChange={checked => edit({ ...value, hidden: !checked }, 'now')}/>}
      <ConfirmAction
        label={tx('settings:bridge.remove-value', { p0: title })}
        title={restores ? tx('settings:bridge.use-the-global-default-for-value', { p0: title }) : tx('settings:bridge.remove-value-from-value-configuration', { p0: title, p1: tx(scope === 'workspace' ? 'settings:extension-detail.workspace' : 'settings:extension-detail.user') })}
        confirm={restores ? tx('settings:bridge.use-global-default-value', { p0: title }) : tx('settings:bridge.remove-value', { p0: title })}
        description={tx(restores ? 'settings:forms.restore-help' : hasModels ? 'settings:models.delete-provider-and-models' : 'settings:forms.delete-help')}
        tone={restores ? 'restore' : 'destructive'} settle={form} onConfirm={() => {
          if (!unit.admitted || locked) return;
          autosave('cancel');
          unit.submit(true);
        }}
        renderTrigger={(open, ref) => <Menu open={menu} align="end" compact autoFocus onClose={() => setMenu(false)}
          anchor={<button type="button" ref={ref} className={cards.iconButton} aria-label={tx('settings:models.provider-actions')} aria-haspopup="menu" aria-expanded={menu}
            aria-disabled={locked} onClick={() => { if (!locked) setMenu(!menu); }}><IconEllipsisOutline16/></button>}
          items={inspecting
            ? [{ id: 'customize', label: tx('settings:forms.customize') }]
            : [
              { id: 'rename', label: tx('settings:models.rename-provider'), icon: <IconEditOutline16/>, disabled: locked || renaming },
              { id: 'icon', label: tx('settings:models.choose-icon') },
              { id: 'credential', label: tx(credential.kind === 'environment' ? 'settings:models.use-key' : 'settings:models.use-environment') },
              ...canDiscardCreation ? [{ id: 'discard-creation', label: tx('settings:copy.delete'), icon: <IconTrashOutline16/>, danger: true }] : [],
              ...canRetry ? [{ id: 'retry', label: tx('settings:models.retry-provider-save') }] : [],
              ...authored ? [{ id: 'delete', label: tx(restores ? 'settings:models.restore' : 'settings:copy.delete'),
                icon: <IconTrashOutline16/>, danger: !restores, disabled: !unit.admitted }] : [],
            ]}
          onSelect={action => {
            setMenu(false);
            if (action === 'rename' && !locked) {
              // Combine the name and any pending connection edits in the next write.
              autosave('hold');
              nameEditing.current = { cancelled: false, composing: false };
              setRenaming(true);
            }
            if (action === 'icon') setIconPicker(true);
            if (action === 'credential') changeCredential(credential.kind === 'environment' ? { kind: 'literal', value: '' } : { kind: 'environment', variable: '' });
            if (action === 'customize') setCustomizing(true);
            if (action === 'discard-creation') { autosave('cancel'); unit.discard(); }
            if (action === 'retry') autosave('now');
            if (action === 'delete') open();
          }}/>}
      />
    </div>
    {iconPicker && <IconPicker value={value.icon} onChange={icon => { if ((icon ?? null) !== (value.icon ?? null)) edit({ ...value, icon }, 'now'); }} onClose={() => setIconPicker(false)}
      finalFocus={() => actionsTrigger() ?? false}/>}
    {inspecting && <p className={cards.inherited}>{tx('settings:forms.using-global')}</p>}
    <fieldset disabled={inspecting} className={cards.connectionFields}>
      <label>{tx('settings:models-page.endpoint')}
        <input type="url" required value={value.base_url} readOnly={unit.awaitingObservation} aria-invalid={attempted > 0 && endpointError}
          placeholder={/* i18n-raw: example URL */ 'https://api.example.com/v1'}
          onChange={event => edit({ ...value, base_url: event.target.value })}/>
      </label>
      <Choice label={tx('settings:models-page.protocol')} value={value.protocol} disabled={locked || inspecting} options={[
        ['openai_chat_completions', tx('settings:models.api-chat')],
        ['openai_responses', tx('settings:models.api-responses')],
        ['anthropic_messages', tx('settings:models.api-anthropic')],
      ]} onChange={protocol => edit({ ...value, protocol }, 'now')}/>
      {credential.kind === 'environment' ? <label>{tx('settings:models-page.environment-variable')}
        <input required value={credential.variable} readOnly={unit.awaitingObservation} aria-invalid={attempted > 0 && credentialError}
          onChange={event => changeCredential({ kind: 'environment', variable: event.target.value })}/>
      </label> : <div ref={keyEditor} className={cards.secret}>
        <div className={cards.keyHeading}>
          <label htmlFor={keyId}>{tx('settings:models-page.new-literal-credential')}</label>
          {getKey && !authored && <a href={getKey} target="_blank" rel="noreferrer">{tx('settings:models.get-key')}</a>}
        </div>
        <input id={keyId} type={keyVisible ? 'text' : 'password'} autoComplete="new-password" spellCheck={false}
          value={key} readOnly={unit.awaitingObservation} aria-invalid={attempted > 0 && credentialError}
          placeholder={credential.kind === 'retain' || inspecting ? tx('settings:models.saved-key-placeholder') : tx('settings:models.enter-key')}
          onChange={event => {
            if (!key) setShowKey(false);
            changeCredential(event.target.value === '' && authored?.credential.type === 'literal'
              ? { kind: 'retain' } : { kind: 'literal', value: event.target.value });
          }}/>
        <Button size="sm" className={cards.iconButton} disabled={!key} aria-pressed={keyVisible}
          aria-label={tx(keyVisible ? 'settings:models.hide-key' : 'settings:models.show-key')} onClick={() => setShowKey(!keyVisible)}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" aria-hidden="true">
            <path d="M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12Z"/><circle cx="12" cy="12" r="3"/>{keyVisible && <path d="m3 3 18 18"/>}
          </svg>
        </Button>
      </div>}
    </fieldset>
    <SettingsNotice occurrence={invalidName} text={invalidName ? tx('settings:models.invalid-provider-display-name') : undefined}/>
    <SettingsNotice occurrence={attempted} text={attempted ? endpointError ? tx('settings:models.invalid-endpoint') : credentialError ? tx('settings:models.invalid-key') : undefined : undefined}/>
    <UnitOutcomeNotice title={title} outcome={unit.outcome}/>
    {unit.reviewNeeded && <div className={cards.connectionActions}>
      <Button size="sm" disabled={locked || !unit.admitted || endpointError || credentialError} onClick={() => {
        unit.review();
        autosave('now');
      }}>{tx('settings:models.apply-provider-changes')}</Button>
    </div>}
  </form>;
}

function connectionErrors({ base_url, credential }: ProviderWrite) {
  let endpointError = true;
  try {
    const url = new URL(base_url);
    endpointError = !['http:', 'https:'].includes(url.protocol) || !url.hostname || !!(url.username || url.password || url.search || url.hash);
  } catch { /* Incomplete or invalid input stays a draft. */ }
  const credentialError = credential.kind === 'literal' ? !credential.value.trim() || credential.value.startsWith('$')
    : credential.kind === 'environment' && !/^[a-zA-Z_][a-zA-Z0-9_]*$/.test(credential.variable);
  return { endpointError, credentialError };
}
