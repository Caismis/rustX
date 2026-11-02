# Attempt 细分共享、借用捕获与索引缩窄

**四项改造已实现，功能检查 2,338 项通过、0 失败。性能结果有改善也有回退：8 轮历史的延迟配对中位数改善 1.572% / 3.139%，64 轮历史则回退 5.109% / 4.987%。完整写入 25 对减少、47 对相同、0 对增加；不能宣称整体提速或追平 Pi。**

本轮直接比较[上一版](2026-10-30-shared-domains-final.md)和当前生产代码，不把不同批次的百分比相减，也不挑掉不利样本。生产提交 `2184461af208af8480a53497a91aabb059c44e16`，src tree `9f7652ed98ab2149da718153c239e5466b3576fb`，普通 O3/default-feature 二进制 SHA256 `8b19873c647647f9eda7a331e071539a18d07f1266ec0e2d1130f8515fc0d2ce`。新运行的旧版基线为 `e9df19d2db38961343d4f05aa7d9c601fecc5968`，src `682c72fbc9d0ce39fd2f11c73555e026effd7ff4`，二进制 `ed788eb3b39027ea9597a9b07b7bda1d4fc9e5eb314096948f40cf1000acde27`。

## 实现与功能边界

1. **Attempt 内部细分所有权。** model、execution settings、foreground 和 in-flight 分开共享，每个 live block 有独立 owner。先只读定位要修改的 block，再分离该 block，保留其他 block 正文与旧 cut。其他 seed 字段和真正变化的正文仍有复制成本。实现见 [shared_attempt.rs](../../src/runtime_client/projection/shared_attempt.rs)。
2. **借用捕获和组合预算。** 已准入 block/Tool 在内部 AttemptView 中借用；model/settings 共享已规范化值。单项结构与 safe-integer 预算延迟计算，并对未变 owner 复用；最终预算逐层组合，包含 Option 和集合开销。实际需要完整公开 checkpoint 时才取得所有权，最终 DTO 和线协议编码仍执行。保留准入顺序、两阶段拒绝、动态装饰、JSON 顺序、signed zero、frontier 和源证据。冷路径直接移动实际规范化分配。实现见 [attempt_view.rs](../../src/runtime_client/projection/attempt_view.rs)、[prepared_attempt.rs](../../src/runtime_client/projection/prepared_attempt.rs)。
3. **复用完整聚合 fold。** 每连接仅保留最新 Statistics/Execution 的规范化完整状态。每次仍读取当前选中 cut 的真实库内字节，并做完整字节比较；保留 hole、native 来源和 provenance 查询。外部改写、损坏、失败写入或回滚使旧缓存无法替代当前事实。显式重现存储 decoder 对共享字段、未存储 activity 字段及非有限 timing 的规范化。公开 receipt 与下次 fold 的所有权分开。实现见 [append_facts.rs](../../src/durable/sqlite/append_facts.rs)、[observation_statistics.rs](../../src/durable/sqlite/observation_statistics.rs)、[observation_execution.rs](../../src/durable/sqlite/observation_execution.rs)。
4. **删除索引内重复的尾部 sequence。** 保留全部独立 native/Trace 索引，只删除非唯一索引中重复的尾部 INTEGER PRIMARY KEY；由隐含 rowid 保持同等顺序和范围能力。唯一索引、sequence 单列索引和 sequence 前导覆盖布局保留。新增结构检查要求 sequence 为真正 rowid 别名，拒绝 INT PRIMARY KEY、INTEGER PRIMARY KEY DESC、WITHOUT ROWID 和复合主键。[SQLite 格式文档](https://www.sqlite.org/fileformat.html)说明普通索引记录仍附加 rowid，即使 INTEGER PRIMARY KEY 已显式入索引；生产所用 SQLite 的回归进一步检查真实覆盖/range plan、历史 cut 与 dbstat payload。

WAL/NORMAL、原有持久性和主库提交边界不变，每个普通热轮次仍为 5 次主库写提交；事件、检查接口和独立派生域没有删减。按仓库规则，schema 升至 **87**，旧版数据库会被拒绝，没有迁移或兼容层。

## 功能与实验验证

最终 15 步检查绑定 `checks-final-2`：**2,338 passed、0 failed、1 项既有 ignored**，fmt 和 release/all-target/all-feature Clippy（`-D warnings`）通过。包含 72 项真实进程死亡、151 项 durable integration、62 项 process integration。新增 12 项回归，审计要求的 282 个不重复关键测试均实际通过。964 个 src/tests/Cargo 输入在检查前后相同，并匹配随后干净 O3 构建的输入。

新回归对照公开 wire 和原全量预算遍历，检查修改单个 block、旧 cut、精确共享预算、unsafe 数值、当前 Tool 装饰、共享 signed zero 的存储规范化、有效/损坏外部改写、回滚及覆盖范围查询。首个完整队列因旧 query-plan 文本、硬编码版本和新增测试的错误 error variant 断言而失败；修正精确断言后重新跑全队列，失败证据仍保留。

正确性使用 release 依赖/root O0/all features；性能随后单独干净构建普通 O3/default features。本任务的编译和检查在正式测量前结束。正式协议：六场景 × 两模式 × 六对，**144 产品、72 对、1,692 请求对照**；CPU 2、8，每格旧→新与新→旧各三对。记录调用线程 affinity 和前后拓扑，子进程继承限制；不宣称全机独占或持续采样每个线程。

**Pi 和 DeepSeek 没有重跑。** 固定引用仍是 Pi Durable `90d863f7c1183b965f099855715d6e88c7b8a52c`（`packages/durable`）与 DeepSeek `687f282e8625c6b8d11ff58211a90524be2e4317`。历史批次未限制 CPU，原值在[完整表格](2026-11-02-incremental-attempt-tables.md)中单列，不作为本轮固定核心数据的新倍数分母。DeepSeek SDK idle 不等待最终 flush/fsync，完整进程写入不可得，且缺少同等 partial-token 发布。Rust/Pi 核心请求等价性排除工具定义，AppServer transport 不属于该进程内基准。

## 正式结果

下表为六对百分比变化的中位数；每产品先算自己的 measured-turn median/mean，再计算配对变化。请求 CPU 仅测量窗口，完整 CPU/写入截止 cleanup。CPU 原计数为 100 Hz，每端点 10 ms。

| 模式 | 历史轮数 | median wall | mean wall | 请求 CPU | 完整 CPU | 完整写入 |
|---|---:|---:|---:|---:|---:|---:|
| 无实时订阅 | 8 | −1.572% | −0.510% | −1.786% | −2.210% | −0.336% |
| 无实时订阅 | 64 | +5.109% | +4.467% | +5.057% | +0.812% | −0.560% |
| 实时订阅 | 8 | −3.139% | −3.925% | −1.515% | −1.526% | −0.348% |
| 实时订阅 | 64 | +4.987% | +5.992% | +8.001% | +1.358% | −0.614% |

8 轮历史两格都是五对延迟下降、一对上升；64 轮历史分别是一降五升、零降六升。热请求各产品 median wall 的批次中位数为 6.986→6.878 / 7.532→7.885 / 7.959→7.744 / 8.702→9.136 ms，和表中的配对百分比不是同一统计量。测量窗口写入也未处处减少：no-live/64 批次每轮中位数为 1,005,056→1,006,336 bytes，完整进程写入仍减少；不能把两个边界混用。

stream 两模式延迟分别 −0.240% / −1.794%，完整 CPU +2.083% / −10.000%，完整写入中位数都为 0%。全十二格的 median wall 为八降四升，逐对 **39 降、33 升**；mean wall 为 41 降、31 升。完整 CPU 为 **24 降、29 同、19 升**；请求窗口 CPU 为 23 降、19 同、30 升。完整写入为 **25 降、47 同、0 升**；RSS 为 **55 降、17 升**，匿名 PSS 为 44 降、28 升。

长尾仍保留：no-live/64 pair 1 的平均延迟 8.254→15.472 ms，**+87.446%**；live/64 pair 2 为 9.209→11.283 ms，+22.516%。no-live/cancel pair 0 为 +51.698%。全部单对反例、范围和不可用比例见完整表格，未重跑筛选不利格，也未把它们归为已解释的噪声。

## 写入诊断与回退调查

独立的四个 columns、四个 WAL 产品也通过 strict audit。每 32 热轮次主库提交均为 **160→160**；WAL frames 为 short-small **5,745→5,724**、short-large **6,444→6,453**。索引逻辑变窄没有带来一致 WAL 下降；逻辑列字节、WAL 帧、进程 write_bytes 和设备流量分别保留。

随后额外运行 **6 个 profiler 产品（4 CPU、2 heap）** 调查 live/64，独立于正式矩阵与上述 8 个诊断，不混池。四个 measured-window CPU profile 分别只有 **94 / 101 / 96 / 98** 个样本；旧版合计 192，新版 197。PublicationDraft::encode 的重叠累计样本分别旧版 17/9、新版 20/19，sqlite3_step 为旧版 23/16、新版 24/11；样本稀疏且波动较大，无法据此确定是哪项改造造成约 5% 的回退。

两个 heap 产品的部分约 1 MiB 堆栈列表，逐行合计不等于 header，故不用于完整函数分配归因。归档中的早期 `baseline-heap-summary.json` 是检查前生成的截断前缀计算，明确不采信为完整统计。经 control log 绑定 `0004→0005`（测量前→测量后）的 header-only 窗口请求分配计数/字节分别旧版 1,220,379 / 317,812,315，新版 1,226,190 / 320,294,436；heap 模式更换 allocator，含 driver/profiler 影响，不能当成普通运行的分配数量或延迟原因。另一个合成 SQLite 3.45.1 probe 的覆盖/范围查询结果相同，缩窄索引每次少一条 VM 指令，仅支持该有限 probe 的结论。

调查没有找到足以解释并消除回退的新增冗余，**64 轮历史回退仍作为观测结果保留**。这四项已落实和验证；最终输出编码、真实来源验证及独立查询维护的成本依然存在，现有证据不支持继续给它们分配精确因果比例。

## 证据交付

[主归档](evidence/incremental-attempt-runtime-20261102/README.md)：1,116 个 payload 文件，10,750,292 压缩 bytes，SHA256 `06d963327a1ef17af141c3fdbb5e1e4923d7936c8cbd323440fe578751f1bb91`。严格身份审计、独立解压逐文件哈希、原始 formal/diagnostic 算术及完整表格比对均通过，见 [portable receipt](evidence/incremental-attempt-runtime-20261102/portable-verification.json)。独立 verifier 已随目录提供，可在新目录复算，不运行产品、编译器或历史引用。

[补充调查归档](evidence/incremental-attempt-investigation-20261102/README.md)：141 个文件，6,103,404 压缩 bytes，SHA256 `7a037b5936beacdb88803cca061e9e2d4174476974fd65b6f2f302617d3010a2`。其独立 verifier 复算 CPU 原始样本总数及 heap 不完整性，保留全部 6 个产品和限制，和主归档的正式结果分开。旧报告、旧证据均未覆盖。未运行全仓库 CI/browser E2E，也未合并 main；PR 原有冲突仍在。

补充归档初次封存后，复核 control log 发现早期 heap 算术使用了初始化→预热的 `0003→0004`，已修正为测量前→测量后的 `0004→0005`。原 [superseded 归档](evidence/incremental-attempt-investigation-initial-20261102/README.md) 的原字节及 SHA `969bff06f064e07dcabfee96d96b336bfe7a86f40a9fe87af71b285ff1395874` 仍保留，仅作为纠错追溯，不采信其测量窗口标签。新版 verifier 强制检查日志阶段名并重算 header 差值；未重跑任何产品，也未改变正式矩阵或 CPU 样本。两个补充 archive 保存的是同一组 6 个产品，不能重复计数。
