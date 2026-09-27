/* Copyright (c) 2026 DeepSeek. MIT. Selected wording adapted; see PROVENANCE.md. */
export const en = {
  "sidebar-root.sidebar": "Sidebar",
  "toggle.open": "Expand Sidebar",
  "toggle.collapse": "Collapse Sidebar",
  "session.new.label": "New Conversation",
  "session.new": "New Conversation",
  "panels.label": "Global panels"
} as const;
export type SidebarKey = keyof typeof en;
export const zh = {
  "sidebar-root.sidebar": "侧边栏",
  "toggle.open": "打开侧边栏",
  "toggle.collapse": "收起侧边栏",
  "session.new.label": "新建会话",
  "session.new": "新会话",
  "panels.label": "全局面板"
} satisfies Record<SidebarKey, string>;
