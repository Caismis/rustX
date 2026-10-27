# Checkpoint、派生读写与发布成本的集中优化

2026-10-27。本轮把上一轮留下的主要重复工作一并实现，并完成同版本功能检查、
276 次正式运行、独立源码/原始数据审计。相对上一版，四个热请求场景的**轮次平均
耗时配对降幅中位数为 17.52%–38.23%**，完整进程 CPU 降低 10.21%–14.53%，
完整进程写入降低约 6%。普通轮次的写事务从 16 次减到 13 次，实际 SQL prepare
调用减少 75.88%。这些是整组修改的结果，不能分摊为每个补丁的独立收益。

**仍未追平 Pi Durable。** 新鲜独立 Pi 对比中，热请求的轮次平均耗时配对比值
中位数为 5.42–6.70×；若沿用上一报告的轮次中位耗时指标，则为 **5.20–8.22×**。
不能把两种指标混称，也不能把历史各轮收益相乘。普通轮次实际 WAL 仍约为 Pi
的 5.63–6.16 倍。剩余差别包括精确 publication 编码、冻结当前状态、现用历史
索引，以及提交后安装/释放协议；没有将这些成本包装成已经消失。

## 源码、构建与边界

| 角色 | Git revision | 普通 O3 executable SHA-256 |
| --- | --- | --- |
| 上一轮直接基线 | `123accb38fd8cf1e8f88583c1858d514cb453870` | `6899c8fad1fc85e8ffd36a33b92e85cec877cda3689bb26a03c6f07c658a3d43` |
| 本轮最终候选 | `f5cf22a8fbd2b1f0b3abc7fc2082a586f1126e20` | `8d033129f343660821b5fd7cef957a424e6d769c55948011f2798c6e2f3a0cb7` |

本轮两个实现提交为 `688e7c3e85a72ae7d3745aff7419fdcff21c4ec8` 和最终候选。
第一候选有独立探索、检查和诊断记录，最终正式数据全部绑定第二个提交及上述
binary，未混池。后续文档提交不改变测量的生产代码。性能构建使用 Rust 1.98.1、
default features、普通 release O3；功能检查使用 release dependencies、根 crate
opt-level 0、all features，不能把功能检查的 executable 当性能构建。

SQLite 仍为 WAL / `synchronous=NORMAL`，与 pinned Pi backend 一致。当前 schema
为 **79**，删除旧格式与无生产调用的 API，不提供迁移或兼容解码。第一候选与最终
候选均属本轮未发布的 schema79，实验使用独立新库，不承诺其间数据兼容。

## 具体消除的额外工作

| 额外工作 | 已实现的改变 | 保留的功能约束 |
| --- | --- | --- |
| 小 delta 也构造完整 checkpoint DTO，随后丢掉正文 | 先捕获借用的规范化域、metadata 与充分预算证据；普通已认证 delta 延迟完整 snapshot 的物化。replacement、显式读取或预算无法证明时才构造一次 | 原有 fit/eviction 顺序、精确 wire 长度、错误与 cursor 发布边界不变 |
| transcript 页 cut 变化使未变 entries 重新序列化、解码、计长 | 删除整页 prepared owner，改为 entries/deferred/statistics 三个独立 owner；当前 cut/envelope 始终取新 seed | 浮点、map 顺序、宽整数、临界预算逐字差分；不把旧 metadata 当当前 cut |
| 同一原子 model settlement 重复安装/释放四次 | provider、assistant canonical、turn/attempt 或 adoption 共用完整 typed family 与 receipt，最后回调一次安装和释放 | 原始事件及独立 observer 顺序保留；缺失/损坏证据或安装失败不得发布 ready |
| append 反复构造、分阶段写入和解码相同派生值 | append-scoped typed facts；同一 failure domain 的 Trace/statistics/members 最多三次 UPDATE 合为一次，inline source/witness 两次合一；final family evidence 合并读取 | 必须先读取实际存储字节并与本次构造结果一致才复用，触发器改写/损坏仍进入真实解码检查 |
| readiness 重复验证和改写，相关读取跨多个锁和事务 | ready/direct 联合有条件更新，已满足时零改写；同 cut 的无 trace live checkpoint 读取从五次锁/三次 read TX 合为一次锁/一次 read TX | 同一 read TX 认证 source cut；显式 trace 路径保留为原语义对照，历史 cut 和错误顺序保持 |
| 固定热点 SQL 反复编译 | 约 134 个固定调用点复用 prepared plans，statement LRU 从 64 调到 256；completion SQL 构造一次 | 仅缓存 SQL plan，不缓存查询结果、历史证据或认证结果 |
| 私有 request inspection 索引重复字段名、编码 hash/bytes、镜像 key 信息 | 使用已有依赖的 bounded positional MessagePack，hash/bytes 用二进制，删除权威 F/C key 已包含的重复字段 | 原始 opaque JSON 字节/数值/顺序不经过 MessagePack Value；原有容量、深度、EOF、结构验证保留 |
| 第三份无现用消费者的事件正文副本 | 删除 `events.observation_json`、`latest_observation_events`、`ObservationEventHeader` 及对应旧展示入口 | 保留现用 witness/Trace/字段源；inherited provenance 的同名字段有独立用途，未删除 |

最后一项对每个 native J 删除一次 bounded 编码、一次 UPDATE 与一对
SAVEPOINT/RELEASE。第一候选保留的 32 轮大历史诊断中，256 个新 J 的旧副本共
101,120 逻辑字节，和该次 native event JSON 逐字相同；不是最终新库中仍有此列。
它不是消除一个顶层 write transaction，不能把 512 条 savepoint 控制
语句说成 512 次 durable commits。现用 source-inline 的 structural JSON 与 native
JSON 对 u64 的编码不同，不能把二者直接别名化。

实现入口：[checkpoint/projection](../../src/runtime_client/projection.rs)、
[prepared captures](../../src/runtime_client/projection/prepared_capture.rs)、
[append facts](../../src/durable/sqlite/append_facts.rs)、
[settlement](../../src/durable/sqlite/final_settlement.rs)、
[receipt](../../src/durable/sqlite/observation_receipt.rs)、
[readiness](../../src/durable/sqlite/observation_readiness.rs)、
[statement cache](../../src/durable/sqlite/statements.rs)。归档中的 `event-body-closure.md`、
`publication-closure.md` 和 `remaining-cost-decision.md` 保存源码追踪与反例。

## Pi 与 DeepSeek 的源码对照

Pi 固定为 `90d863f7c1183b965f099855715d6e88c7b8a52c`。它的
[storage write batch](https://github.com/earendil-works/pi/blob/90d863f7c1183b965f099855715d6e88c7b8a52c/packages/durable/src/storage/sqlite/storage.ts#L187-L209)
提交 entries/tasks/documents，成功后
[adopt 和内存 listener publication](https://github.com/earendil-works/pi/blob/90d863f7c1183b965f099855715d6e88c7b8a52c/packages/durable/src/session/session.ts#L690-L778)
不再另写 J/T ready/direct 状态。其
[answer settlement](https://github.com/earendil-works/pi/blob/90d863f7c1183b965f099855715d6e88c7b8a52c/packages/durable/src/harness/generation.ts#L542-L567)
把 assistant、run 结束和后续输入交接放进一次 commit；本轮采用同一已有原子结算
只发布一次的方式。其
[prompt planner](https://github.com/earendil-works/pi/blob/90d863f7c1183b965f099855715d6e88c7b8a52c/packages/durable/src/harness/prompt.ts#L66)
按改变追加 system/tools，上一轮 rustX 已实现相应配置共用，本轮继续缩小 inspection
索引，而不是再保存一份完整 prompt。

DeepSeek 固定为 `687f282e8625c6b8d11ff58211a90524be2e4317`。其
[agent loop](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/core/agent-loop/src/agent.ts#L626)
跳过未变普通 header；
[JSONL persistence](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/session/session-persistence-jsonl/src/index.ts#L1411-L1438)
批量 append 后调用真实 `handle.sync()`，
[request checkpoint policy](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/session/session-checkpoint-policy/src/index.ts#L29-L36)
在 dispatch 前显式 flush。不能把 JSONL batch 和 SQLite TX 当相同单位。

三引擎参考使用 pinned 原生 SDK/原生工具和 session，经 TSX 加载；保留 loader
profile 调整与 dependency identity。**SDK idle 不等待最终 flush/fsync**，不提供
partial-token-text publication，因此它是额外成本参考，不能据其较低 write_bytes
声称等持久性优势或作内置 CLI 启动排名。12 次 close/resume 全部通过，但可读恢复
不能证明断电持久性或 fsync 成功；独立 flush 耗时与完整退出后写入计数不可用。

## 正式性能结果

每个 no-live/live × 六场景固定五对。下面是五个逐对差值百分比的中位数，wall 使用
每个产品的 `mean_turn_ms`；CPU/写入包含初始化、历史、验证及 cleanup，不等同于
单轮测量区间。流式/大响应/工具/取消同样全部展示。
本文“完整 CPU/写入”专指截至 `after_cleanup` 的进程累计端点；CPU 为 self CPU，
包含同进程驱动，不包含外部 provider，也不是进程退出后的 `wait4` 全生命周期计数。

| 模式 | 场景 | wall 均值变化 | 完整 CPU 变化 | 完整进程写入变化 |
| --- | --- | ---: | ---: | ---: |
| no-live | 热请求，小历史 | -22.37% | -13.69% | -6.08% |
| no-live | 热请求，大历史 | -21.11% | -11.52% | -6.18% |
| no-live | 流式 | -0.54% | -3.12% | -5.80% |
| no-live | 大响应 | -11.82% | -5.56% | -6.24% |
| no-live | 工具 | -16.78% | -14.29% | -6.57% |
| no-live | 取消 | -14.79% | 0.00% | -5.92% |
| live | 热请求，小历史 | -17.52% | -10.21% | -6.03% |
| live | 热请求，大历史 | -38.23% | -14.53% | -6.12% |
| live | 流式 | -1.50% | -6.67% | -5.80% |
| live | 大响应 | -0.78% | -11.11% | -6.24% |
| live | 工具 | -14.51% | 0.00% | -6.43% |
| live | 取消 | -0.82% | -5.88% | -5.92% |

12 格 wall 均值配对中位数都降低，并不表示每个样本或每项指标都改善。live 大历史
匿名 PSS 中位数 +0.12%，live tool +0.33%；有单对 wall、CPU 和内存回退。
live 取消虽然单轮 settled wall 中位变化为 -0.82%，但截至产品退出的 wall 配对
中位数为 +4.48%，完整 harness 生命周期为 +4.23%，这两项中位回退同样保留。
完整逐对范围、所有不利项和不可用值保留在
[自动生成的完整表](2026-10-27-complete-costs-tables.md)，没有剔除不利样本。
五对仅提供描述统计，不满足此前七对完整追平门槛。

新鲜 Pi 队列与直接比较队列分开，不合并两批 rustX：

| 模式 | 热请求历史 | rustX / Pi 平均 wall | rustX / Pi 中位 wall | 完整 CPU | 完整进程写入 |
| --- | --- | ---: | ---: | ---: | ---: |
| no-live | 小 | 5.678× | 6.901× | 1.574× | 6.797× |
| no-live | 大 | 5.888× | 5.200× | 3.121× | 7.649× |
| live | 小 | 5.423× | 6.537× | 1.844× | 6.799× |
| live | 大 | 6.696× | 8.224× | 4.013× | 7.649× |

同一新鲜队列中，Pi 的产品内中位 wall 再取五产品中位数为 3.27–3.56 ms/turn，
rustX 为 17.01–29.11 ms/turn。单轮 measured CPU：Pi 6.56–7.50 ms，rustX
20.00–37.50 ms。两列范围没有按端点相除。其他八格 rustX/Pi wall 配对中位数
为 0.382–0.963×；这不抵消热请求与写入差距。

独立三引擎 live 小/大历史参考中，产品内平均 wall 的三产品中位数分别为 rustX
27.24/26.40 ms、Pi 3.91/4.02 ms、DeepSeek SDK idle 15.13/14.69 ms。所有四场景、
三引擎的 CPU/写入/范围和退出边界单列在完整表中，未计算等持久性 DeepSeek 比率。

## 逻辑字节、真实 WAL 与 SQL

以下是 8/64 个历史轮次之后、32 个测量轮次的独立诊断。列字节是同步数据库快照
间的逻辑值增量；WAL 是 strace 原始写入重新解析的 commit markers/frames，不是
按 JSON 缩小量预测。process write_bytes、WAL bytes 和物理设备 I/O 含义不同。

| 量 | 小历史：旧 → 新 | 大历史：旧 → 新 |
| --- | ---: | ---: |
| 全部表逻辑列字节 | 2,564,940 → 1,770,302 (-30.98%) | 2,672,096 → 1,826,076 (-31.66%) |
| request pages/nodes/roots 逻辑字节 | 1,369,729 → 676,185 (-50.63%) | 1,465,449 → 720,804 (-50.81%) |
| WAL write commits | 512 → 416 | 512 → 416 |
| WAL frames | 9,892 → 9,475 (-4.22%) | 10,831 → 10,406 (-3.92%) |
| WAL frame bytes（含 frame header） | 40,755,040 → 39,037,000 | 44,623,720 → 42,872,720 |
| 同 workload Pi commits / frames | 160 / 1,683 | 160 / 1,688 |

每轮 write TX 为 16→13，Pi 为 5。同一事务内脏页会合并，独立索引与提交又会重复
写页，因此约 31% 的逻辑缩小只对应约 4% 的 WAL 缩小。最终候选相比第一候选
两格各多 9 个 frames；没有把删除旧正文单独声称为 WAL 实测收益。

匹配的带 transaction 栈 SQL probe 中，prepare 调用从 **18,738 降到 4,520**，
即 585.5625→141.25/turn，下降 75.88%。较早 plain baseline 的 18,682 来自不同
读取事务交错，未偷换分母。缓存命中执行不会再次调用 prepare，故该计数不是 SQL
执行总量。新 `BEGIN IMMEDIATE` 为 416，独立 WAL commit markers 也是 416；
不能把包括 read TX 的 804 次 COMMIT preparation 当 write commits。GDB/strace
诊断耗时不进入正式性能统计。

剩余 4,520 次中，1,992 次属于事务控制，2,528 次来自 45 种尚未缓存的固定业务
SQL。后者可继续逐点复用，并非 SQL 编译已全部消除；本轮采样仍命中其读取 owner。
这属于剩余 CPU 空间，不减少 WAL 或语义事务，不能推算成已实现的收益。

## 剩余差距的具体来源与取舍

13 次普通写事务由 **7 次语义提交 + 6 次安装后释放**组成：接受 Pending Inbound、
adopt、AttemptStarted、TurnStarted、冻结请求、打开输出 stream、最终 settlement。
除打开 stream 外各有现用安装/释放边界；最终 settlement 从四次释放合成一次。
这不是理论下限，也不等同于额外 fsync。两边已使用相同 WAL/NORMAL 策略。

rustX 的独立 WAL reader 可以观察「Journal 已提交但投影尚未安装」；普通 reopen
保留部分释放状态，新 observer 恢复才执行恢复协议。当前 native commit → 安装成功
→ 持久 ready/direct release → 唤醒读者的顺序，防止安装失败或尚未安装的事实被
读者误认为发布成功。提前标记、只放内存、延迟到下一语义提交，都会改变这些结果。
ready/direct 已在紧凑独立表，普通 release 不重写宽 events 正文，不能把迁出宽行
当新机会。位图也仍要保存洞、direct 与后置提交；本轮没有引入未验证的新状态格式。

native event、structural field source、bounded witness 和 Trace 分别承担原始执行
权威、字段 seek/字节切片、有限预览/故障生存、历史关系与 owner 认证。已删的是
无消费者且字节相同的旧显示副本。将现用层合并成单一失败域会改变读取/损坏行为；
按需从 native 重建 source 则会把编码转移到读取，并非无代价删除。

checkpoint 在预算证明通过的普通 delta 路径不再物化完整 DTO，但仍冻结 bounded working snapshot，并对
真正发布事件执行一次 schema-aware 精确计长，以维持 replay 预算、淘汰与错误
边界。原始 u64 的 string 编码使普通 serde 长度不能替代。剩余的 immutable owner
共享与精确片段计长仍是优化空间，未声称实现了所有可能优化；当前证据不能支持
以一次全面 owner/serializer 重写换取已知整体追平收益。

新鲜单次 250 Hz live 大历史 profile 的 baseline/final 为 314/312 样本。
publication 为 61/62，其中 capture 24/22、精确计长 30/36、其他 7/4；
`read_with_trace` 38/29，SQLite prepare 63/18。明确 snapshot clone 为 6/21，
最终约 6.7% 的样本，其中 capabilities 占 15；这是保留的真实复制工作。
**本次 profile 没证明 publication 总 CPU 降低**，不能引用第一候选较低的采样值
代替最终候选。采样命中不是调用次数、重建次数或 wall-time 占比。

capability 原始 header 的单域 immutable owner 共享，比全域重构更聚焦，尚未实施。
现有 prepared head 只保存规范化的 bounded page，不能未经证明取代原始历史分页
authority（包括超限/无效 header、offset 和精确字节）。这和剩余固定 SQL 是真实的
后续优化空间；本轮没有为了一个样本中数个百分点的潜在 CPU 收益继续改动已完成
验证的候选，也没有声称这些成本必然不可优化。

同一 profile 的实际 consumer notification 增量为 546→451，与 family 合并相符，
但不是所有 publication/read 调用数。正常 workload 仍有 transcript entries 初始化，
不推翻 cut-only 差分测试的零重建，也不能用其推算整体收益。最终保留决策来自
预先固定的正式矩阵和正确性检查，未将 profile/control 或探索运行混入。

## 功能验证与可复算证据

最终 source-bound 检查通过 **1,383 tests，1 ignored**，另有 fmt 和
all-target/all-feature Clippy `-D warnings`。覆盖 SQLite、Runtime Client、
conversation runtime、read-layout、content、durable 与 process；检查前后源码
hash 相同并与最终候选一致。新增检查覆盖 lazy 构造次数、wire/预算差分、
transcript 当前 cut、超八条 adoption、独立 WAL reader、安装失败、整批 rollback、
历史 cut、coherent corruption、巨大原始事件/ID 的 bounded witness，以及存储字节
被 trigger 改写时不能错误复用。失败队列、修正测试 fixture 的原因和重新运行全部保留。

正式范围为直接对比 120、fresh Pi 120、三引擎 36 个产品，共 **276**；前两队列
分别完成 60 个比较、1,410 次请求检查，三引擎完成 12 个比较、414 次请求检查。
同引擎使用完整 request oracle；跨引擎使用共同语义，因为原生 prompt/tool inventory
不同。两个候选各 24 次探索及各 11 次诊断产品与正式结果分开保留。旧基线诊断另有
原始来源记录，不把所有 probe 数量混称为新的候选测试。

[证据目录及复算说明](evidence/complete-costs-runtime-20261027/README.md) 包含完整
原始输入、脚本、精确命令、build identity、检查历史、CPU stacks、SQL 和 WAL
原始记录、DeepSeek native session 及 manifest。strict auditors 核对实际 executable、
当前 src/tests/Cargo、全部实验维度与 oracle；归档后再从新目录解包，逐文件 hash
验证并重新执行两个 raw-only auditors，比较所有正式指标、admission 和诊断结果。
portable 模式明确不宣称重新验证历史主机 executable。

归档已实际完成新目录验证：[验证记录](evidence/complete-costs-runtime-20261027/portable-verification.json)
为 accepted，2,802 个 payload 文件逐字节 hash 一致，276 个正式产品的指标、准入、
请求比较及全部诊断复算与 strict 报告一致。`raw.tar.xz` 为 39,816,596 字节，SHA-256
`c3fa3cbdfbdd7055b97627fe51acdb03a0e3692b0fd2270a687d771d9dc811e1`。

CPU `/proc` 计数粒度 10 ms；PSS 是采样最大值；process writes 不是设备 I/O。
未运行全仓库 CI/browser E2E，未合并 main；PR 保持 draft，既有 main 冲突仍在。
