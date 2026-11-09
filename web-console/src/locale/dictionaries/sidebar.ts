/* Copyright (c) 2026 DeepSeek. MIT. Selected wording adapted; see PROVENANCE.md. */
export const en = {
  "sidebar-root.sidebar": "Sidebar",
  "toggle.collapse": "Collapse Sidebar",
  "toggle.dismiss": "Close navigation",
  "session.new.label": "New Conversation",
  "session.new": "New Conversation",
  "navigation.label": "Primary navigation",
  "navigation.home": "Home",
  "navigation.schedules": "Scheduled Tasks"
} as const;
export type SidebarKey = keyof typeof en;
export const zh = {
  "sidebar-root.sidebar": "侧边栏",
  "toggle.collapse": "收起侧边栏",
  "toggle.dismiss": "关闭导航",
  "session.new.label": "新建会话",
  "session.new": "新会话",
  "navigation.label": "主导航",
  "navigation.home": "主页",
  "navigation.schedules": "计划任务"
} satisfies Record<SidebarKey, string>;
