export type LocaleId = 'en' | 'zh';
export interface LocaleSnapshot { readonly active: LocaleId; readonly revision: number }
export const LOCALE_STORAGE_KEY = 'rustx-locale-v1';
export const documentLanguage = (locale: LocaleId) => locale === 'zh' ? 'zh-CN' : 'en';

/** Browser capabilities are injected so denied/missing APIs have the same safe
 * behavior as an ordinary page. No native client is reachable from this owner. */
export interface LocaleEnvironment {
  read(): string | null;
  write(locale: LocaleId): void;
  languages(): readonly string[];
  documentLanguage(language: string): void;
}
const browser: LocaleEnvironment = {
  read: () => localStorage.getItem(LOCALE_STORAGE_KEY),
  write: locale => localStorage.setItem(LOCALE_STORAGE_KEY, locale),
  languages: () => typeof window === 'undefined' ? [] : navigator.languages?.length ? navigator.languages : [navigator.language],
  documentLanguage: language => { if (typeof document !== 'undefined') document.documentElement.lang = language; },
};
export class LocaleController {
  private snapshot: LocaleSnapshot;
  private readonly listeners = new Set<() => void>();
  constructor(private readonly environment: LocaleEnvironment = browser) {
    let active: LocaleId | undefined;
    try { const stored = environment.read(); if (stored === 'en' || stored === 'zh') active = stored; } catch { /* Optional browser preference. */ }
    if (!active) {
      try { active = /^zh(?:-|$)/i.test(environment.languages().find(tag => tag.length > 0) ?? '') ? 'zh' : 'en'; } catch { active = 'en'; }
    }
    this.snapshot = Object.freeze({ active, revision: 0 });
    this.syncDocument();
  }
  getSnapshot = (): LocaleSnapshot => this.snapshot;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  setLocale = (active: LocaleId): void => {
    // Runtime callers cannot widen the two-locale contract.
    if (active !== 'en' && active !== 'zh') return;
    try { this.environment.write(active); } catch { /* A denied write does not prevent live switching. */ }
    if (active === this.snapshot.active) return;
    this.snapshot = Object.freeze({ active, revision: this.snapshot.revision + 1 });
    this.syncDocument();
    for (const listener of [...this.listeners]) listener();
  };
  private syncDocument() { try { this.environment.documentLanguage(documentLanguage(this.snapshot.active)); } catch { /* Non-browser rendering. */ } }
}
/** The single page owner, created once outside React lifecycle. */
export const localeController = new LocaleController();
