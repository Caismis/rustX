import {interactionBody} from '../../../protocol/app-server/interaction.ts';
import type { RuntimeClientInteractionHeader } from '../../../protocol/app-server/v57.ts';
import type { RoutedInteraction } from '../protocol/app-server.ts';
/** A missing native body is a deferred display row, never an empty request. */
export type DisplayInteraction = RoutedInteraction | RuntimeClientInteractionHeader;
export function displayInteraction(header: RuntimeClientInteractionHeader): DisplayInteraction {
  return interactionBody(header) ?? header;
}
export function hasInteractionBody(value: DisplayInteraction): value is RoutedInteraction {
  return 'request' in value;
}
export function interactionKind(value: DisplayInteraction) {
  return hasInteractionBody(value) ? value.request.kind.type : value.kind;
}
