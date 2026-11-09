import { SettingsNotice } from '../SettingsFeedback';
import { useRef, useState } from 'react';
import type { ProviderWrite, SourceMutation } from '../../../../../protocol/app-server/v77';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { DialogSurface } from '../../../presentation/primitives/DialogSurface';
import { IconCloseOutline16 } from '../../../presentation/primitives/icons';
import { useUnitEditing } from '../forms/bridge';
import { ProviderIcon } from './ProviderIcon';
import css from '../../../presentation/settings/SettingsContent.module.css';
import cards from './ModelsPage.module.css';

interface ProviderTemplate {
  name: string;
  icon: string;
  protocol: ProviderWrite['protocol'];
  url: string;
  keyUrl: string;
}

// API-key templates adapted from ZCode's config/provider/zcode-builtin.json.
export const providerTemplates: readonly ProviderTemplate[] = [
  { name: 'BigModel', icon: 'zhipu', protocol: 'openai_chat_completions', url: 'https://open.bigmodel.cn/api/paas/v4', keyUrl: 'https://bigmodel.cn/usercenter/proj-mgmt/apikeys' },
  { name: 'BigModel-Coding-Plan', icon: 'zhipu', protocol: 'anthropic_messages', url: 'https://open.bigmodel.cn/api/anthropic', keyUrl: 'https://bigmodel.cn/coding-plan/personal/overview' },
  { name: 'DeepSeek', icon: 'deepseek', protocol: 'anthropic_messages', url: 'https://api.deepseek.com/anthropic', keyUrl: 'https://platform.deepseek.com/api_keys' },
  { name: 'OpenAI', icon: 'openai', protocol: 'openai_responses', url: 'https://api.openai.com/v1', keyUrl: 'https://platform.openai.com/api-keys' },
  { name: 'Anthropic', icon: 'anthropic', protocol: 'anthropic_messages', url: 'https://api.anthropic.com/v1', keyUrl: 'https://console.anthropic.com/settings/keys' },
];

export function providerMutation(id: string, authored: ProviderWrite | null, restore = false): SourceMutation {
  return { kind: 'config', mutation: restore && authored === null ? { unit: 'restore_provider', id } : { unit: 'provider', id, authored } };
}

export function ProviderNameDialog({ exists, revision, onCreate, onClose }: {
  exists: readonly string[]; revision: string; onCreate: (id: string) => void; onClose: () => void;
}) {
  const tx = useTranslation();
  const [identity, setIdentity] = useState('');
  const [template, setTemplate] = useState<ProviderTemplate>();
  const input = useRef<HTMLInputElement>(null);
  const name = identity.trim();
  const duplicate = exists.includes(name);
  const invalid = /[\s/]/.test(name);
  const blank: ProviderWrite = {
    icon: template?.icon, protocol: template?.protocol ?? 'openai_chat_completions',
    base_url: template?.url ?? '', credential: { kind: 'literal', value: '' },
  };
  const unit = useUnitEditing({ blank, revision, mutation: value => providerMutation(name, value) });
  const title = tx('settings:models-page.add-provider');
  return <DialogSurface open onClose={onClose} title={title} overlayClassName={cards.overlay} className={`${css.settings} ${cards.nameDialog}`} initialFocus={input}>
    <div className={cards.dialogHeader}><h2>{title}</h2><Button aria-label={tx('settings:models.close')} onClick={onClose}><IconCloseOutline16/></Button></div>
    <form onSubmit={event => { event.preventDefault(); if (name && !duplicate && !invalid) { unit.edit(blank); onCreate(name); } }}>
      <div className={cards.templates} aria-label={tx('settings:models.templates')} role="group">
        <Button size="sm" variant="outline" aria-pressed={!template} onClick={() => { setTemplate(undefined); setIdentity(''); }}><ProviderIcon size={14}/>{tx('settings:models.custom-template')}</Button>
        {providerTemplates.map(item => <Button key={item.name} size="sm" variant="outline" aria-pressed={template === item}
          onClick={() => { setTemplate(item); setIdentity(item.name); }}><ProviderIcon icon={item.icon} size={14}/>{item.name}</Button>)}
      </div>
      <label>{tx('settings:models-page.new-provider-identity')}
        <input ref={input} required value={identity} aria-invalid={duplicate || invalid || undefined} onChange={event => setIdentity(event.target.value)} />
      </label>
      <SettingsNotice text={duplicate ? tx('settings:models.duplicate') : invalid ? tx('settings:models.invalid-provider-name') : undefined}/>
      <div className={cards.nameActions}><Button onClick={onClose}>{tx('settings:aria.cancel')}</Button><Button type="submit" variant="primary" disabled={!name || duplicate || invalid}>{tx('settings:models.continue')}</Button></div>
    </form>
  </DialogSurface>;
}
