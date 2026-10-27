# 复用已认证 owner，压缩重复正文与长度索引

2026-10-27。本轮实现了四项结构优化，并完成了功能验证和 RustX 新旧版本实测。**逻辑存储增长减少 10.72%–12.07%，实际 WAL frames 仅减少 0.54%–0.80%；整体延迟和 CPU 尚无一致改善，仍未追平 Pi Durable。** 不能把删除重复表示直接等同于显著的端到端收益。

按用户要求，本轮 **没有运行 Pi 或 DeepSeek Harness**。后续比较使用上轮已经验证、固定版本的参考数据。新旧 RustX 的交替配对结果与历史 Pi/DS 的跨批次参考分开报告。

## 实现与功能边界

| 原有重复工作 | 当前实现 | 保留的约束与限制 |
| --- | --- | --- |
| 每个请求子树保存 80 项精确长度 | 保存精确分段长度及条目数；按 journal/rank 数字宽度重建对应长度 | 80 种宽度类别仍逐一精确，保留导航、预算和 native digest 绑定；不接受旧 dense 格式 |
| 小事件的原始正文和结构化 source 各存一份完整文本 | source 共享 native `event_json`，仅保存长度、双摘要和结构化整数的插入引号位置 | 读取仍认证完整字节、位置和最终结构；增加摘要/重建计算。小 source 现在依赖原始正文，正文损坏会导致该派生读取失败；原生恢复不依赖派生格式 |
| 发布时反复深复制元数据、重新计算完全相同的精确长度 | 不可变元数据 owner 共享值及长度证书；native binding head 复用当前路径映射，frontier 仅比较固定字段 | wire/schema、旧 cut、非法整数及超预算处理保留。变化的路径/值重新构造；event/envelope 与实际输出仍须编码 |
| checkpoint 刷新时重读已完成响应的 execution/member/closing/retry | 连接内最多保留 32 项有界的已认证结果；相同 native owner 在有效证据下复用 | 仍检查当前 cut、成员/执行缺失证据、统计和外部变更；运行中、证明不完整的继承/terminal-only 路径不准入，写事务临时结果不准入 |

源码入口：[精确长度表示](../../src/durable/sqlite/observation_request/composed/width_table.rs)、[事件 source overlay](../../src/durable/sqlite/observation_event/inline/overlay.rs)、[元数据 owner](../../src/runtime_client/checkpoint/metadata_fragment.rs)、[发布 fragment](../../src/runtime_client/projection/prepared_capture.rs)、[响应证明复用](../../src/durable/sqlite/observation_response/cache.rs)。

事件 source 写入仅在可精确表示且满足原有大小边界时选择 overlay；其他合法形状使用现有分段表示。已经声称存在的 overlay 若缺失、篡改或格式错误则失败，不以其他表示掩盖损坏。公开 source 读取使用同一读事务；真实双连接回归覆盖查询之间发生外部正文修改的情形。

响应缓存观察同连接 preupdate、外部 data version、main/temp schema version，以及实际咨询过的 J/L/T 依赖。它保留旧 cut 的原生检查和错误顺序，DDL 后停止复用该连接的证明。新增工作量回归证明：8 项已完成响应跨更大 J 读取时，可省掉 native owner SQL 及 execution/member 解码，仍保留当前执行缺失证据查询。**这是特定路径的实际计数证明，不代表运行时所有刷新均命中。** 后台读到落后于数据库的 cut 时，冷项不能取得当前 head 的准入证明；本轮 profile 没有记录该情况的发生率。

## 版本与验证

| 角色 | 生产源码提交 | 普通 O3/default-feature executable SHA-256 |
| --- | --- | --- |
| 上轮 Pi-flow RustX | `d22ce66c5a1cda895cac382ce2c350f345933876` | `775b6426d1ce59e1f80e746000d52527e876917f117361841ce120b183d042bb` |
| 本轮 owner reuse | `459b1b6005bfbf7f0a1a1d52df5a6900b06c56c6` | `21d50ee8ac86bb80a2132e6013ece360ae56944d311abbaf87c0576ef32c0e57` |

后续报告提交不改变生产源码。SQLite 继续使用 WAL/NORMAL，schema 从 80 升为 **81**，没有迁移或兼容解码。

最终 `checks-retry5` 的 13 步通过：**2,109 项测试通过，1 项按既有设置忽略**，格式检查及 all-target/all-feature Clippy `-D warnings` 通过。包括 72 项真实进程死亡测试、151 项 durable 集成测试、62 项 process 集成测试，以及新的 26 项回归。全部 926 个 src/tests/Cargo 输入在检查前后保持一致。失败编译、失败用例、Clippy 修复及先前检查队列均保留，没有用探索性检查代替最终完整队列。

正确性队列采用 release dependencies、rustX package O0、all features；性能采用独立 clean 后的普通 O3/default-feature 构建，Rust 1.98.1。没有执行完整仓库 CI/browser E2E，也没有合并 main；PR 既有冲突仍待处理。

## 正式配对结果与不利样本

两种观察模式 × 六场景 × 五对，共 120 个 RustX 产品运行。每对交替先后顺序，使用相同 harness、provider 脚本和请求 oracle。独立严格审计通过全部 60 对比较、1,410 次请求对应关系。以下是逐对百分比的中位数；wall 是每个产品内部轮次中位耗时。CPU/写入端点为 `after_cleanup`，包含初始化、历史、验证和关闭，并非单轮或进程退出后的 wait4。

| 模式 | 场景 | 中位 wall 变化 | 完整 CPU 变化 | 完整进程写入变化 |
| --- | --- | ---: | ---: | ---: |
| no-live | 热请求，小历史 | -5.32% | -0.70% | -1.12% |
| no-live | 热请求，大历史 | -5.17% | -0.23% | -1.12% |
| no-live | 流式 | -0.64% | -6.45% | -0.23% |
| no-live | 大响应 | +3.88% | 0.00% | -0.26% |
| no-live | 工具 | -0.14% | -5.88% | -0.40% |
| no-live | 取消 | +7.44% | 0.00% | -0.26% |
| live | 热请求，小历史 | +1.16% | -0.56% | -1.13% |
| live | 热请求，大历史 | +20.36% | +8.58% | -1.11% |
| live | 流式 | +1.13% | +3.57% | -0.23% |
| live | 大响应 | +1.34% | 0.00% | -0.26% |
| live | 工具 | +2.04% | +5.88% | -0.40% |
| live | 取消 | -8.98% | -6.25% | -0.26% |

live 大历史的正式 wall 配对范围为 **-44.14% 到 +83.36%**。由于中位方向不利，又执行了独立的 7 对同场景检查；它不替换、不混入正式五对。补充批次 wall 中位变化为 **-8.63%**，范围 **-46.59% 到 +80.30%**，完整 CPU 为 **-4.26%**。两批方向相反，现有证据不足以给出稳定的提速或回退归因；也不能直接把不利结果称作噪声。没有按有利方向选择结果后删除另一批。

内存并非全面下降：正式 live 小/大历史匿名 PSS 配对中位为 -2.31%/-2.29%，但 live 流式/大响应/工具分别为 +0.70%/+0.29%/+0.50%。所有指标、绝对值、范围、每个不利配对及不可用比值见[自动生成的完整表](2026-10-27-owner-reuse-tables.md)和归档 `report-data.json`。100 Hz 的进程 CPU 计数具有 10 ms 粒度，不能把短用例的小整数跳变解释成精确的单函数成本。

## 真实存储与 WAL

独立列快照/WAL 产品均在 8 或 64 个历史轮次后测量 32 轮；没有与正式耗时产品混池。逻辑列字节、WAL frame bytes、process write_bytes 是不同指标，不等同于设备 I/O。

| 指标 | 小历史：旧 RustX → 本轮 | 大历史：旧 RustX → 本轮 |
| --- | ---: | ---: |
| 主库逻辑列字节增量 | 1,406,272 → 1,255,565 (-10.72%) | 1,453,470 → 1,277,977 (-12.07%) |
| request pages/nodes/roots 逻辑增量 | 313,648 → 245,251 | 349,764 → 256,726 |
| request node metadata 逻辑增量 | 124,877 → 56,480 | 164,199 → 71,161 |
| 小 source 表示，含旧 inline 长度列 | 102,302 → 19,968 | 102,400 → 19,968 |
| 主库 WAL 写提交 | 160 → 160 | 160 → 160 |
| WAL frames | 7,109 → 7,052 (-0.80%) | 7,977 → 7,934 (-0.54%) |
| WAL frame bytes，含 frame header | 29,289,080 → 29,054,240 | 32,865,240 → 32,688,080 |

相比上次固定 Pi 的 1,683/1,688 frames，本轮仍为 **4.19×/4.70×**；这是跨批次参考。缩小正文/节点表示只减少了部分页，仍保留的历史、content、事件派生字段及索引在多次提交中触及大量页。端点 dbstat 标签不能精确归因每一次中间写页，不能把表的逻辑字节下降按比例换算成 WAL 收益。TEMP 就绪状态不在主库快照内，其缺席没有被算成零内存。

## 与固定 Pi、DeepSeek 参考比较

Pi 固定提交 `90d863f7c1183b965f099855715d6e88c7b8a52c`，DeepSeek 固定提交 `687f282e8625c6b8d11ff58211a90524be2e4317`。具体源码调用链和持久性差异见[上轮源码报告](2026-10-27-pi-flow.md#pi-与-deepseek-的源码依据)。本轮参考绑定上次已接受的严格审计及原始归档 SHA-256；不以历史数据验证新 RustX 的功能或源码。

以下是新 RustX 五产品中位数除以上次 Pi 五产品中位数，**不是新鲜配对比值**，不能直接与前轮逐对比值的中位数混用。

| 模式 | 热请求历史 | mean wall / 固定 Pi | median wall / 固定 Pi | 完整 CPU / 固定 Pi | 完整写入 / 固定 Pi |
| --- | --- | ---: | ---: | ---: | ---: |
| no-live | 小 | 4.914× | 5.729× | 1.457× | 4.744× |
| no-live | 大 | 4.940× | 5.554× | 2.993× | 5.230× |
| live | 小 | 5.506× | 6.166× | 1.845× | 4.744× |
| live | 大 | 4.474× | 4.746× | 3.353× | 5.230× |

热请求产品内中位耗时的五产品中位数为 RustX **16.803–21.087 ms/turn**，固定 Pi **3.135–3.541 ms/turn**。流式、大响应、工具及取消的全部参考比值同样保留，不只展示热请求。

DeepSeek 只使用上次四个 live 场景、各三次的数据。小/大历史 mean wall 为当前 RustX **21.659/17.847 ms**、固定 DS **15.541/15.450 ms**，比值 **1.394×/1.155×**。DS SDK idle 不等待最终 flush/fsync，也没有 partial-token-text publication；完整退出后的 write_bytes 不可用。其 wait4 退出/flush CPU 在证据中单列，不能将 SDK idle 比值声称为同持久性或同生命周期比较。

## 为什么收益仍有限

本轮大历史 live profile 为旧版 **260**、候选 **252** 个样本。`PublicationDraft::encode` 含子调用计数 **61→58**；其互斥分解为 capture **22→39**、末端精确长度 **37→16**、其他 **2→3**。旧 metadata 末端整块编码样本消失，但首次 fragment 计量仍有成本；整个 transcript 页变化后仍会重建其 capture，相关样本 **9→16**。样本不能当作调用次数、独立补丁收益或总 CPU 改善证明。

响应 `read_validated` 为 **22→25**，checkpoint read 为 **31→39**；preupdate hook 两版各只有 **1** 个样本，不支持把整体回退归因到这个 hook。独立 control 的测量区间 CPU 从 91 到 108 ticks，采样产品从 103 到 102 ticks，也没有一致的改善方向。

仍可削减的工作有明确来源：

- transcript 的 prepared owner 仍以整页为单位；一个 row 或 response 装饰变化会使整页 capture 重建。后续若改成行 owner，必须同时保留行顺序、内容 locator、预算降级及旧 cut，不能只按 ID 复用可变化的整行。
- cut 验证、响应缓存准入与 statistics 在同一事务仍会重复读部分 J/L/T 前沿。已有本地 statistics row 的完全命中非空响应页通常仍有 **8+H** 次 SQL（不含外层 cut 验证，H 为保留的执行缺失查询），还要解码统计；seed/bootstrap 统计分支会额外读取，checkpoint 另读 transcript headers、occupancy 和 projection。
- witness、Trace、execution、members、statistics 仍重复保存部分身份、时间和终态字段，并分阶段生成更新。用途不同不意味着现有完整快照表示不可继续合并。
- Trace raw-binding 刷新在结果不变时仍可能重写 summary。源码证明存在重复，不等于已测出它是主要 WAL 来源。

因此，当前结果支持“删掉了若干冗余表示及特定路径的重复计算”，不支持“所有大头已经消除”或“保留功能即可保证追平 Pi”。继续优化应针对这些真实路径的工作量和失效边界，不能再次把完整 checkpoint 构造笼统列为每次发布必做；前轮的 lazy checkpoint 物化仍然有效。

## 证据与复算

本轮保存 120 个正式产品、12 个隔离诊断产品，以及因回退而追加的 14 个 live 大历史产品。三组不混池，所有运行均为 RustX。构建、失败检查、源码身份、采集命令、原始 CPU/WAL/列快照、独立审计及固定参考哈希随[证据目录](evidence/owner-reuse-runtime-20261027/)保留，排除编译二进制。

归档大小、SHA-256、完整文件清单和全新目录 raw-only 复验结果见证据目录的 `summary.json`、`portable-verification.json`；补充批次独立复验另列。旧 Pi/DS 原始数据仍是显式固定依赖，未伪装成本轮重新采集。

归档为 **12,583,652 bytes**，保留 **1,134** 个 payload 文件，SHA-256 为 `dcee5a053499af6e65f32c2523272f5bc1ca2f55d0262399cbdde68d4986516c`。全新解压目录核对全部文件后，正式/诊断审计和报告数值重算与原结果一致；补充 14 个产品的独立 raw-only 重放也通过，原始文件在两次复验前后保持不变。
