/** The in-memory preview workspace owns these aggregate presentation budgets. */
export const PREVIEW_POLICY = Object.freeze({
  tabsPerSession: 8, retainedSessions: 4, panes: 2, activeBodies: 2,
  pdfWorkers: 2, downloads: 1, urls: 3,
  paneMinWidth: 300, dividerWidth: 8, minRatio: .2, maxRatio: .8,
});
