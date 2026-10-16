# 架构导览

在桌面浏览器中直接打开 [`index.html`](index.html)。页面适合宽度至少 1200px 的桌面窗口，采用宽图和右侧说明栏；较小窗口可以横向滚动。它是一个可复制的单文件页面，通过 `file://` 打开即可使用，其中包含全部样式、脚本、说明与 source 摘录。无需服务器、软件包、账号或网络连接，需要启用 JavaScript。可选的 GitHub 链接需要网络连接。

本导览展示 `main` 的固定 snapshot **`8cb41624eb2dc2ca87e4409961f42553ceb029cf`**。它不会执行请求、读取本地配置或跟踪工作区变化。总览按 ownership 分组，卡片表示代码中承担 responsibility 的模块，并不表示部署单元或进程。

选择卡片会选中节点并显示右侧说明，其中列出 responsibility、boundary、关系及可展开的 source 摘录。需要继续浏览时，点击说明栏中的“进入内部图”或“进入外部模块”按钮。外部 owner 以虚线外框和跨模块提示区分，进入后可沿返回路径回到原处。请求讲解、场景与专题中的紧凑卡片和节点标签会转到架构总览并选中对应节点，之后可从说明栏进入模块。内部图以两列分组充分展开模块，显示当前关系过滤下的全部箭头，并用粗线突出所选节点的关系；总览只追踪所选模块。图中的箭头与右侧清单区分运行时调用、数据流和代码依赖；清单端点可先选中节点，再从说明栏进入下一层。

面包屑可以回到所属模块或总览，“返回上一处”沿实际访问路径返回，包括跨模块跳转和从请求讲解、场景、专题进入内部图。返回会恢复该处选中的节点、关系过滤、打开的 source 摘录及滚动位置。顶部模式切换会保留各模式的步骤和选择。

带点状下划线的 English term 可悬停或通过键盘聚焦，弹出的中文说明解释 rustX 中的具体情况。按钮内的 term 跟随按钮获得键盘说明；普通正文中的 term 可独立聚焦。鼠标可移入说明框继续阅读，Escape 关闭说明。source 代码、路径和标识符不会被自动标注。

在仓库根目录中使用 Node.js 和 Git 重新构建：

```bash
node docs/architecture-explorer/build.mjs
node docs/architecture-explorer/build.mjs --check
```

无需安装软件包。本地 Git 对象数据库必须包含固定提交。`build.mjs` 合并 `content.json` 的讲解、`architecture.json` 的内部图，以及 `glossary.json` 与内部图的 term 定义，通过 `git show` 从精确提交提取 source 行，再与 `styles.css`、`app.js` 一起嵌入 `index.template.html`。输出不使用 `HEAD` 替代基线，链接也指向同一提交。

`content.json` 管理总览、请求、场景与专题。`architecture.json` 的 `entries` 必须为每个总览模块提供一个入口；每个 view 声明 `parent`、`owner`、`nodes` 和 `relations`。节点的 `target` 指向其子图；指向其他模块入口时必须同时声明 `external: true`。没有 `target` 的节点必须是可用 source 依据解释的实现末端。层级必须无环，每个子图都要有来自 parent 的入口。

term 仅在作者明确写下 marker 时展开，不会通过全局文字替换猜测标注。正文示例：

```json
{
  "summary": "用户取消请求后，{{owner}} 先停止推进，再等待 {{drain}} 完成。"
}
```

`glossary.json` 为数组，元素包含唯一的 `id`、English `label` 和中文 `explanation`；机制专用定义也可写入 `architecture.json` 的 `terms`。解释应描述 rustX 何时发生该动作、由哪个组件负责、影响什么结果，不能只给出字典翻译。不同语境应使用各自的 term ID。不要在代码、路径或标识符内部使用 marker。

`--check` 验证内容并比较预期输出与已提交的 `index.html`，不会写入文件。构建拒绝重复或未知 ID、缺失入口、错误 target、循环 parent、未知或格式错误的 marker、重复 term 定义、不安全的 source 路径、无效行号和超过 80 行的摘录。输出具有确定性，不包含时间戳或环境路径，JSON 会安全转义后嵌入 script。

编辑内容、交互或样式后重新构建，并将生成的 `index.html` 与源文件一起提交。如需修改固定 snapshot，需同时更新 `content.json` 与 `build.mjs` 的精确提交，重新检查全部说明和 source 范围，并更新本文档。
