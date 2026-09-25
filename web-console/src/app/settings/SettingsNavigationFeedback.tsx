import { useSelector } from '@xstate/react';
import type { SettingsNavigationActor } from './machines/navigation';
import { Button } from '../../presentation/primitives/Button';

/** Owner lookup failures belong beside the Session's configuration action. */
export function SettingsNavigationFeedback({ navigation }: { navigation: SettingsNavigationActor }) {
  const error = useSelector(navigation, snapshot => snapshot.context.error);
  return error && <p role="alert" aria-label="Settings navigation error">{error}<Button size="sm" onClick={() => navigation.send({ type: 'DISMISS' })}>Dismiss settings error</Button></p>;
}
