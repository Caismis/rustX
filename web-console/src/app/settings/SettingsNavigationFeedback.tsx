import { displayText } from '../../locale/translation';
import { useTranslation } from '../../locale/react';
import { useSelector } from '@xstate/react';
import type { SettingsNavigationActor } from './machines/navigation';
import { Button } from '../../presentation/primitives/Button';

/** Owner lookup failures belong beside the Session's configuration action. */
export function SettingsNavigationFeedback({ navigation }: { navigation: SettingsNavigationActor }) {
  const tx = useTranslation();
  const error = useSelector(navigation, snapshot => snapshot.context.error);
  return error && <p role="alert" aria-label={tx('settings:settings-navigation-feedback.settings-navigation-error')}>{displayText(tx, error)}<Button size="sm" onClick={() => navigation.send({ type: 'DISMISS' })}>{tx('settings:settings-navigation-feedback.dismiss-settings-error')}</Button></p>;
}
