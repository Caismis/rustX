import { createContext, useContext, type ReactNode } from 'react';
/** Only an explicit document owner can supply images or workspace-link actions.
 * Conversation Markdown has no provider and keeps its inert image policy. */
export const DocumentResources = createContext<{
  image: (url: string, alt: string) => ReactNode;
  link: (url: string, children: ReactNode) => ReactNode | undefined;
} | undefined>(undefined);
export function DocumentImage({url,alt}: {url:string;alt:string}) {
  const resources=useContext(DocumentResources);
  return resources?.image(url,alt) ?? <span>{alt}</span>;
}
export function DocumentLink({url,children,fallback}: {url:string;children:ReactNode;fallback:ReactNode}) {
  const resources=useContext(DocumentResources);
  return resources?.link(url,children) ?? fallback;
}
