/* Copyright (c) 2026 DeepSeek. MIT. Selected wording adapted; see PROVENANCE.md. */
export const en = {
  "tool-card.copy": "Copy",
  "tool-card.copied": "Copied",
  "tool-card.collapse-output": "Collapse output",
  "tool-card.collapse": "Collapse",
  "tool-card.show-value-more-lines": "Show {p0} more lines",
  "tool-card.tool-status": "Tool status",
  "tool-card.exit-status-unavailable": "Exit status unavailable",
  "tool-card.running": "Running",
  "tool-card.failed": "Failed",
  "tool-card.done": "Done",
  "tool-card.no-output": "No output",
  "tool-card.requested-changes": "Requested changes",
  "tool-card.in": "IN",
  "tool-card.out": "OUT",
  "copy.signal-value": "Signal {p0}",
  "copy.exit-value": "Exit {p0}",
  "copy.value-file-s": "{p0} file(s)",
  "copy.value-of-value-lines": "{p0} of {p1} lines"
} as const;
export type ToolsKey = keyof typeof en;
export const zh = {
  "tool-card.copy": "复制",
  "tool-card.copied": "已复制",
  "tool-card.collapse-output": "收起输出",
  "tool-card.collapse": "收起",
  "tool-card.show-value-more-lines": "展开其余 {p0} 行",
  "tool-card.tool-status": "工具状态",
  "tool-card.exit-status-unavailable": "退出状态不可用",
  "tool-card.running": "运行中",
  "tool-card.failed": "失败",
  "tool-card.done": "已完成",
  "tool-card.no-output": "无输出",
  "tool-card.requested-changes": "请求的更改",
  "tool-card.in": "输入",
  "tool-card.out": "输出",
  "copy.signal-value": "信号 {p0}",
  "copy.exit-value": "退出码 {p0}",
  "copy.value-file-s": "{p0} 个文件",
  "copy.value-of-value-lines": "{p1} 行中的 {p0} 行"
} satisfies Record<ToolsKey, string>;
