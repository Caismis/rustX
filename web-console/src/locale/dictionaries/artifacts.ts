/* Copyright (c) 2026 DeepSeek. MIT. Selected wording adapted; see PROVENANCE.md. */
export const en = {
  "artifact.preview": "Preview",
  "attachment-card.open-image-value": "Open image {p0}",
  "attachment-card.loading": "Loading…",
  "attachment-card.image-attachment": "Image attachment",
  "attachment-card.file-attachment": "File attachment",
  "attachment-card.retry": "Retry",
  "attachment-card.load-attachment": "Load attachment",
  "attachment-card.download": "Download",
  "attachment-card.remove-value": "Remove {p0}",
  "attachment-card.close-dialog": "Close dialog",
  "artifact-preview.artifact-preview": "Artifact preview",
  "artifact-preview.wrap-lines": "Wrap lines",
  "artifact-preview.reading-native-artifact": "Reading native artifact…",
  "artifact-preview.retry-preview": "Retry preview",
  "artifact-preview.this-artifact-has-no-supported-inline-viewer": "This artifact has no supported inline viewer.",
  "artifact-preview.download-artifact": "Download artifact",
  "copy.image-could-not-be-decoded": "Image could not be decoded",
  "preview.close": "Close Artifact preview"
} as const;
export type ArtifactsKey = keyof typeof en;
export const zh = {
  "artifact.preview": "预览",
  "attachment-card.open-image-value": "打开图片 {p0}",
  "attachment-card.loading": "正在加载…",
  "attachment-card.image-attachment": "图片附件",
  "attachment-card.file-attachment": "文件附件",
  "attachment-card.retry": "重试",
  "attachment-card.load-attachment": "加载附件",
  "attachment-card.download": "下载",
  "attachment-card.remove-value": "移除 {p0}",
  "attachment-card.close-dialog": "关闭对话框",
  "artifact-preview.artifact-preview": "制品预览",
  "artifact-preview.wrap-lines": "自动换行",
  "artifact-preview.reading-native-artifact": "正在读取原生制品…",
  "artifact-preview.retry-preview": "重试预览",
  "artifact-preview.this-artifact-has-no-supported-inline-viewer": "此制品没有受支持的内嵌查看器。",
  "artifact-preview.download-artifact": "下载制品",
  "copy.image-could-not-be-decoded": "无法解码图片",
  "preview.close": "关闭产物预览"
} satisfies Record<ArtifactsKey, string>;
