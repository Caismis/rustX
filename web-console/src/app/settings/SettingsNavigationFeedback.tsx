import { displayText } from '../../locale/translation';
import { useTranslation } from '../../locale/react';
import { useSelector } from '@xstate/react';
import type { SettingsNavigationActor } from './machines/navigation';
import { Toast } from '../../presentation/primitives/Toast';
import { IconWarningOutline16 } from '../../presentation/primitives/icons';

/** Owner lookup failures belong beside the Session's configuration action. */
export function SettingsNavigationFeedback({ navigation }: { navigation: SettingsNavigationActor }) {
  const tx = useTranslation();
  const error = useSelector(navigation, snapshot => snapshot.context.error);
  return error && <Toast label={tx('settings:settings-navigation-feedback.settings-navigation-error')} text={displayText(tx, error)} holdMs={6000} icon={<IconWarningOutline16/>} actions={[{ label: tx('settings:settings-navigation-feedback.dismiss-settings-error'), onClick: () => navigation.send({ type: 'DISMISS' }) }]} onDone={() => navigation.send({ type: 'DISMISS' })}/>;
}
