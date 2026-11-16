# 共享 read-cut 元数据与合并复用

**本轮已落地元数据映射共享、read-cut 预算复用和有序合并缓存，2,346 项功能检查通过。实时订阅热请求的延迟配对中位数改善 4.681% / 1.731%；无订阅热请求基本持平。写入没有实质下降，实时流式场景延迟和 CPU 有回退，不能宣称整体提速或追平 Pi。**

生产提交 `a664c5a96546c3e49c5638c3c959b08df3c77fec`，src tree `dd0b2c579159d532420979209157e94d32d80131`，干净普通 O3/default-feature 二进制 SHA256 `62c2e96a5a611de0df64e35065ae9bfd04a794bafef882a2214244ff1cb4587e`。直接基线为[上一版生产代码](2026-11-02-incremental-attempt.md) `2184461af208af8480a53497a91aabb059c44e16`，src `9f7652ed98ab2149da718153c239e5466b3576fb`，二进制 `8b19873c647647f9eda7a331e071539a18d07f1266ec0e2d1130f8515fc0d2ce`。

## 落地的改造

1. **种子辅助映射共享。** 将仍会整表复制的 head bindings、source frontiers 和 live frontiers 改用现有依赖 imbl 的有序持久映射；PreparedCaptures 使用相同表示。捕获 seed 共享根节点，修改时只分离变化的树路径。ContentMetadata 和 DomainSources 在此前已经共享，不计为本轮新成果。
2. **read-cut 映射和预算共享。** domain revisions 与 collection cuts 使用 `MetadataFragment<ReadCutMap<V>>`，工作投影、seed 和已发出的 cut 共享不可变 owner。未变 owner 复用 exact 编码长度与 safe-integer 预算；每次修改均使证书失效，即使 Arc 只有一个 owner。外层 header 每次重算，组合时保留字段、Option 和对象开销；穷尽解构让新增字段必须作出预算决定。完整最终输出仍编码。
3. **合并 source frontier 时索引名称。** 从逐 source 扫描所有输出改为借用名称的索引；保留首个重复项优先、源 map 的顺序和源 key 不等于 value.domain 时的原有行为。零/单 source 保留简单路径。
4. **复用原始 evidence 合并。** 每个 metadata cache 只保留最新一项，绑定输入的精确顺序与 owner/head 身份。相同输入复用去重和路径合并结果，随后仍按当前 source 重新解析。不会把缓存的旧事实提升为当前来源，也不保存无限历史链。
5. **相同 collection proof 不触发写时复制。** 只有 proof generation 实际前进或首次出现才更新映射，避免无变化时使共享 owner 和预算失效。

实现见 [projection.rs](../../src/runtime_client/projection.rs)、[read_cut.rs](../../src/runtime_client/checkpoint/read_cut.rs)、[metadata_fragment.rs](../../src/runtime_client/checkpoint/metadata_fragment.rs)。复用已有 imbl 7.0.2 并启用 serde，没有升级依赖版本。公开 Rust 字段表示发生变化；透明 JSON 和 JSON Schema 与原普通有序对象一致，有直接 oracle 回归。按仓库规则不增加旧 API 兼容层。

SQLite 仍为 schema **87**、WAL/NORMAL，持久化表示、提交边界、事件、独立派生域和来源检查均保留。普通热请求每轮仍为 5 次主库写提交。

## 功能与实验边界

最终 15 步队列 `checks-final-3`：**2,346 passed、0 failed、1 项既有 ignored**，fmt 和 release/all-target/all-feature Clippy（`-D warnings`）通过。覆盖 SQLite 637、runtime client 614、AppServer 43、conversation runtime 96、read layout 26、content 27、agent 585、tools 19、recovery 14、真实 process-death 72、durable integration 151、process integration 62。新增 8 项回归，要求的 290 个不重复关键测试均实际通过。966 个 src/tests/Cargo 输入在检查前后相同，并绑定随后干净 O3/default-feature 构建。

回归检查旧 cut 不变、共享根、唯一 owner 修改后缓存失效、当前来源重新解析、重复 domain/path 优先级、原 wire/schema、精确预算和大 key/unsafe 数值边界。早期编译、fmt、Clippy 失败和新增测试的无效 ConversationId/缺失 schema 描述断言均保留；修正后重新完成全队列。

正确性使用 release 依赖/root O0/all features；性能使用单独干净 O3/default features。编译、检查和正式产品运行窗口不重叠。正式矩阵为六场景 × 两模式 × 六平衡顺序配对：**144 产品、72 对、1,692 请求对照**。CPU 2、8，各格旧→新和新→旧各三对。记录控制线程 affinity 与拓扑，子进程继承限制；不宣称全机独占或持续观测每个子线程。

另外运行 **144 个针对性消融产品**，调查此前长历史回退，和 **8 个 columns/WAL 诊断产品**；本轮共 **296 个新 Rust 产品**。消融与正式矩阵分开审计、分开报告，不能合并为同一总体。

**Pi 和 DeepSeek 没有重跑。** 固定 Pi Durable `90d863f7c1183b965f099855715d6e88c7b8a52c`、DeepSeek `687f282e8625c6b8d11ff58211a90524be2e4317` 的 60/12 个历史产品单列于[完整表格](2026-11-02-shared-metadata-tables.md)。历史批次 CPU 未限制，不用于本轮的新倍数或追平声明。DeepSeek SDK idle 不等待最终 flush/fsync，完整进程写入不可得，缺少同等 partial-token 发布；Rust/Pi 核心等价性排除工具定义（7 对 4）。AppServer transport 不属于进程内性能基准。

## 正式结果

以下为六对百分比变化的中位数；每产品先算自己的 measured-turn median/mean，不把所有 turn 混池。请求 CPU 仅测量窗口，完整 CPU/写入截止 cleanup。CPU 原计数为 100 Hz，端点粒度 10 ms，尤其单轮场景比例受粗粒度影响。

| 模式 | 历史轮数 | median wall | mean wall | 请求 CPU | 完整 CPU | 完整写入 |
|---|---:|---:|---:|---:|---:|---:|
| 无实时订阅 | 8 | −0.006% | −0.014% | −1.852% | 0.000% | −0.006% |
| 无实时订阅 | 64 | −0.053% | +0.199% | +1.724% | +0.979% | +0.018% |
| 实时订阅 | 8 | −4.681% | −6.112% | −5.682% | −2.476% | −0.017% |
| 实时订阅 | 64 | −1.731% | −0.984% | −3.697% | +0.753% | +0.036% |

无订阅两格各三对 wall 下降、三对上升；实时订阅 8-history 六对均下降，64-history 四降二升。四格 median wall 的批次中位数分别为 **6.812→6.845 / 7.560→7.558 / 8.165→7.693 / 8.848→8.758 ms**，与配对百分比是不同统计量。

十二格 median wall 为九降三升；逐对 **48 降、24 升**。完整 CPU 为 **16 降、30 同、26 升**，完整写入 **10 降、52 同、10 升**；采样 RSS **48 降、24 升**，匿名 PSS **42 降、30 升**。写入整体基本未变，不能把此轮的内存共享解释为磁盘压缩。

不利结果同样保留：实时 stream wall **+1.317%**（两降四升），请求窗口 CPU **+19.091%**、完整 CPU **+4.026%**；无订阅 stream wall +0.036%、完整 CPU 0%。实时 large wall +0.604%、完整 CPU +3.571%。无订阅 large 虽然 wall 六对均下降、中位 −2.539%，请求 CPU 中位仍为 +33.333%；单轮计数较粗不构成忽略这些上升的理由。实时 large pair 1 的 lifecycle wall +95.862% 也保留。全部反例和范围见完整表格，未重跑筛除不利格。

这些数据支持实时热请求有小幅改善，不支持全场景 CPU 降低。共享和工作计数回归证明对应重复操作被消除，但正式矩阵测的是合并后的实现，不能给每项优化分配精确因果收益。

## 长历史回退调查与写入

[单项消融表](2026-11-02-metadata-ablations.md)包含五个普通 O3 单项反事实构建：强制完整 Attempt detach、提前物化 Attempt、恢复全量预算遍历、关闭聚合 fold 缓存、恢复索引尾部显式 sequence；另加更旧 Rust 生产代码作重现对照。每项两模式、每格六对，CPU 2、8，变体顺序按 pair 轮换、模式顺序交替；144 产品、72 对、6,912 请求对照独立审计通过。

没有发现两模式都稳定更快的禁用项。更旧 Rust 对当前基线的 wall 配对中位数为 +0.707% / +0.448%，两模式各三降三升，因此此前约 5% 回退没有在此批次稳定重现；这不解释或抹除[此前不利矩阵](2026-11-02-incremental-attempt.md)，也不把跨批次百分比相减。恢复显式 sequence 仍使完整写入约 +0.62%，其无订阅单对 −41.853% wall 极值同样保留。

四个 columns、四个 WAL 产品审计通过。32 个热轮次主库提交均 **160→160**；WAL frames 为 8-history **5,718→5,716**、64-history **6,446→6,460**。request snapshots、request source pages、native snapshot 的逻辑字节在对应前后产品中相同，derived overlay 均为 19,968 bytes。没有可归因于本轮的磁盘写入削减。逻辑列字节、WAL 帧、进程 write_bytes 和设备 I/O 不混用；main-only columns 不包括连接本地 TEMP。

## 审计修正与交付

测量前冻结 17 个脚本的哈希，原 formal 审计器仍要求此前 11 个名称，导致第一次审计失败。**17 个原脚本的实际哈希全部未变。** 保留原脚本与失败 `direct-audit.json`，新增 corrected 审计器，只扩充名称集合；diagnostic 审计器只更新指向 corrected formal 的绑定。原始产品、协议、数据选择、语义判断及统计计算均未修改，未重跑产品。`audit-correction.json` 保存精确 diff 和哈希，pack 和 portable verifier 强制验证这一精确变换。corrected 审计器属于测量后修正，不声称其预先冻结。

[原始证据归档](evidence/metadata-runtime-20261102/README.md)包含 **2,106 个 payload 文件**，压缩 **24,107,580 bytes**，SHA256 `dadcf09cd1e4bb8c9588fc6441741482e7ec0399cf41372f385be95b8883c08c`。严格身份审计和[独立 portable 复验](evidence/metadata-runtime-20261102/portable-verification.json)均通过：安全解压后逐文件核对哈希，重新计算全部 formal、消融、columns/WAL 及表格，并在复算后再次确认原始文件无变化。独立 [verifier](evidence/metadata-runtime-20261102/verify-archive.py) 已随归档提供；复验不运行产品、编译器或历史引用。失败检查和审计、所有不利样本、构建与源码补丁均保留，编译缓存、实验 checkout 和可执行文件不入归档。

全仓库 CI/browser E2E 和 main 合并未执行，PR 保持 draft，原有 main 冲突仍在。以前的报告、归档及不利数据均未覆盖。
