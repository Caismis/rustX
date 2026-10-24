import type { RuntimeClientMessageDescriptor, RuntimeClientTranscriptEntry, RuntimeClientTranscriptPage } from '../../../protocol/app-server/v57';
import { messageDescriptor } from '../../../protocol/app-server/message-descriptor';
import { canonicalLocatorKey } from '../../../protocol/app-server/content-identity';

/** Frozen history keeps its own native cut; live typed heads supersede its cache. */
export function deferredToolDescriptors(
  current: readonly RuntimeClientMessageDescriptor[],
  page: RuntimeClientTranscriptPage,
  loaded: readonly RuntimeClientTranscriptEntry[],
  historical: boolean,
): RuntimeClientMessageDescriptor[] {
  const rows = new Map<string,RuntimeClientMessageDescriptor>();
  for(const item of page.deferred_entries ?? []) {
    const descriptor = messageDescriptor(item.value);
    if(descriptor)rows.set(canonicalLocatorKey(descriptor.locator),descriptor);
  }
  if(!historical)for(const descriptor of current)rows.set(canonicalLocatorKey(descriptor.locator),descriptor);
  const knownIds = new Set(loaded.flatMap(entry=>entry.item.type==='message'?[entry.item.message.id]:[]));
  const knownLocators = new Set(loaded.flatMap(entry=>(entry.tool_calls??[]).map(tool=>canonicalLocatorKey(tool.occurrence.locator))));
  return [...rows.values()].filter(descriptor=>!knownLocators.has(canonicalLocatorKey(descriptor.locator))
    && (descriptor.identity == null || !knownIds.has(descriptor.identity)));
}
