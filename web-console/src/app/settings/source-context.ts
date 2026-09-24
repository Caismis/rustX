import { createContext } from 'react';
import type { SourceSettings } from '../../../../protocol/app-server/v21';

/** The authoritative projection the enclosing Settings instance currently
 * holds. Editors read native facts from it; it is never authority and is never
 * merged, recomputed or written back. */
export const SourceContext = createContext<SourceSettings | undefined>(undefined);
