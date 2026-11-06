# PR #485：集成 main

本次仅办理 main 集成和功能验收，性能优化范围保持冻结。合并前分支为 `2b2604103b11d43f525fe4b646619dccb5483482`，集成目标为 `a81d053f7fed8295d347149696430a2e26e6c26e`。Pi、DeepSeek 产品和性能基准均未运行；本次不对集成后的耗时、CPU、内存或写入量作新的比较性结论。

## 集成边界

main 已拆分 SQLite、Agent execution、Session 和 Runtime Client 等模块。性能实现移入对应的新 owner，保留 main 的 Auxiliary request owner、Workflow、配置发布、取消、历史读取和工作区能力；没有用旧的大文件替换 main 的新模块。

- 共享 `MetadataFragment`、持久 read-cut 和已准备 transcript 行下移到原生 observation 层。原生读取不依赖 Runtime Client；准备缓存、checkpoint 和 replay 继续共享不可变正文。
- 保留请求配置共享、Statistics 自包含历史记录、完整 key 导航页、事件单次 INSERT、最终回答原子结算、按需观察、行共享和容量收紧，以及 SQLite 的 30 秒／120 秒闲置回收。
- main 的取消和事务失效不能作为普通可选布局错误降级。prepare、Source 和 occupancy 的可选值路径也传播这两类错误；普通派生域损坏仍独立失败。SQL plan 复用不缓存查询结果或当前存储证明。
- Auxiliary Call／Request 的原生事件行先于依赖索引写入，原生 request owner 校验保留。配置事实保留 main 新增的 transcript seat 和独立显示语义。
- 冻结请求的公开字段采用 main 的正向 allowlist，统一实际 23 字段的构造与校验。response schema、Tool associations 仍由原生 Request Snapshot 冻结，不因不在公开 Trace allowlist 内而丢失。
- main 的完整 Trace 历史保留；PR 的独立 Trace 修复、共享行和 refreshed 前缀确认适配新的客户端读取控制器。读取失败保留在 Trace 域，旧 cut 和当前权威检查继续生效。

唯一当前协议为 App Server **73**、Runtime Client **84**、SQLite **97**。Rust、schema、TypeScript 和客户端同步更新；旧开发数据库与旧客户端被版本 gate 明确拒绝，不增加双读、兼容分支或迁移框架。`InteractionPending` 的大型 header 装箱，保持原有 JSON 表示。

## 验证

| 范围 | 结果 |
| --- | --- |
| Rust 核心、SQLite、Session、Runtime Client、容量与确定性合同 | 5,447 通过，9 个既有忽略 |
| Rust 进程／持久化／MCP／子代理边界 | 262 通过，1 个既有忽略 |
| 外部 conformance／contracts／durable／process／provider／subagent／tools | 658 通过，10 个既有忽略；协议文档的旧版本文字修正后，contracts 31 项复测通过 |
| 4 个 bin 和 7 个 example 的测试入口 | 28 通过 |
| Rust 文档测试 | 9 通过 |
| Linux FAST／FULL 测试 lane 实际 Cargo/libtest 发现 | 三个执行 job 覆盖检查通过 |
| Web 单测 | 4,049 通过，4 个既有跳过 |
| TUI | 1,323 通过，1 个既有跳过 |
| dev、基准工具 Python、CI Python、provider fixture Python | 分别 42、48、64、55 通过；未运行比较产品 |
| Fmt、全目标全 feature Clippy（warnings denied）、TypeScript、i18n、provenance、Web build | 通过 |
| 浏览器 | 最终四分片全量重跑：639 个不同用例全部通过，无失败、重试、跳过或 runner error；参考图未更新 |

回归中另修复了已确认 Attach 在后续观察读取断线后被误判成未知 mutation 的竞态。状态机保留已经确认的阶段；原来真正丢失 Attach ACK 的未知结果合同不变。新增两个确定性顺序测试，与原有生命周期、连接和模型选择测试合计 53 项通过；原超时的窄屏确认丢失用例在锁定浏览器下复测通过。

重连的完整 Trace 头可能包含短 checkpoint 未携带的更早前缀。合并后的最老记录决定分页 cursor 的来源，不能把短头的 `null` 当作历史已完整；Trajectory 的初始加载还会观察 cursor、epoch、loading/error 这些标量的变化，只有 Trajectory 模式触发相应补页，Chat 不承担这些新增触发。新增边界回归及原有 Trace 读取组 44 项通过，真实聊天／历史／图片／重连场景复测通过。

浏览器连接助手避免对已经相同的模式、端点和凭据重新发送 DOM change 事件：它们曾在显式 Connect 手势前提前启动一次恢复，随后又被手势替换。保留真正未知 Attach 结果不自动重放的状态机合同。延迟诊断也保留原始 Trace 过期错误，并且仅在同一 attachment 的后续 head 读取成功后认可恢复；分页失败、跨 claim、损坏或最后一次未恢复错误仍拒绝。相关证据规则 59 项测试及两个真实浏览器测量产品复测通过，不据此给出性能比较。

长流式浏览器夹具也补齐了实际发出的 `conversation/contentRead`：读取已发出的不可变前缀，严格保持引用、字节边界和分块，并新增页面错误断言。200 个 chunk 的原有布局、滚动和行身份断言保持，定向复测通过。

本机为 Linux，Rust 1.98.1；最终 Web 单测及浏览器使用 Node 24.22.0。Web 全量复测使用 4 个 worker、15 秒测试外框，未放宽任何产品内部截止时间或测试中的时序／资源断言。此前共享主机下两个默认 5 秒外框超时保留在工作日志中。浏览器使用仓库锁定的 Playwright 1.63.0 镜像与既有断言，参考图未更新；本地临时启动配置仅复用预先编译的 native addons，避免同一 checkout 的并发 node-gyp 重建，测试目录、配置、每分片 worker 数和像素比较规则均沿用仓库设置。

依赖按 frozen lockfile 安装，未为本地验证改动锁文件；本机 Corepack 实际调用 pnpm 11.28.5，项目声明为 11.13.1，因此不能声称使用了完全相同的包管理器版本。最终 source/test/configuration 摘要、完整检查日志及四份浏览器 JSON 报告保存在[验收证据目录](evidence/main-integration-20261106/README.md)，可按 manifest 校验。

历史性能报告保持原状，不能用它们替代集成后的功能验证或宣称本次的性能收益。macOS／其他平台与推送后的合并门禁由 CI 单独验证。
