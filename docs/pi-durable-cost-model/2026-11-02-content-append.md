# 紧凑内容键与流式尾块追加

**本轮热请求完整写入减少约 0.60%–0.86%，frontier 列值缩小约 20%。延迟结果混合：三个热格改善 1.299%–4.229%，实时订阅 64-history 回退 3.277%；全矩阵逐对 wall 为 35 降、37 升，不能宣称整体提速或追平 Pi。主检查 2,350 项及额外 artifact lineage 8 项均通过，0 失败。**

生产提交 `274d5d47e6e791cd9093aa40b64965def6d178fc`，src tree `4caff9545b0e8f1761509049e82edb8c97f1613e`，干净普通 O3/default-feature 二进制 SHA256 `714f6df872d02e8a90fe97a2becd155a0810c1e7d10301810968031ca619c4af`。直接基线为[上一版](2026-11-02-shared-metadata.md) `a664c5a96546c3e49c5638c3c959b08df3c77fec` / src `dd0b2c579159d532420979209157e94d32d80131` / 二进制 `62c2e96a5a611de0df64e35065ae9bfd04a794bafef882a2214244ff1cb4587e`。

## 实现与功能边界

1. **合并尾块追加。** 原路径 SELECT 旧尾块到 Rust Vec，再扩容追加和 UPDATE。新路径是一条缓存的、带类型和长度条件的 UPDATE；`rustx_append_content` 借用 SQLite 的两个 BLOB 输入，以一个精确容量的有界缓冲区合并。减少单独的旧尾块返回、Vec 增长和 SQL 查询/准备；检查尾块类型、长度和更新行数后才发布下一 frontier。MAX frontier 查询也使用现有 statement cache。SQLite 内部仍会复制数据和写页，不能把它称为零复制或设备级原地追加。
2. **压缩固定私有键。** 四种 representation 使用 `t/r/b/j`；canonical block/message metadata 和 Journal 内容源使用 `cb/cm/oj`。采用固定短码，无字典查询或新配置。公开 serde enum 名称、JSON/Schema、内容和历史 revision 不变，所有私有 SQL 选择器与故障注入同步更新。
3. **共享树节点内的键。** read-cut 映射使用 `Arc<str>`，写时复制路径共享键；已有域通过借用键推进 revision，不再为查找而构造新 String。溢出拒绝、预算证书失效和已发布旧 cut 不变的行为保留。

实现见 [content.rs](../../src/durable/sqlite/content.rs)、[read_cut.rs](../../src/runtime_client/checkpoint/read_cut.rs)、[representation keys](../../src/runtime_client/content.rs)。没有添加依赖。SQLite 版本升为 **88**，按仓库规则拒绝旧格式，没有迁移、双读或兼容路径。WAL/NORMAL 和原提交边界不变，未删除独立派生域、原始字节或来源/损坏校验。公开 Rust 映射键表示改变，透明线协议保持一致。

最初尝试的 SQL 文本拼接在新回归中无法完整保留 UTF-16 数据库的奇数字节序列，已删除，改用严格 BLOB 函数；失败测试和独立 probe 均保留。新回归在项目自带 SQLite 上覆盖 UTF-8、UTF-16le/be、全部 256 种字节、跨 64 KiB 边界、历史前缀、缺失/截短/类型损坏尾块、trigger 失败和回滚。另一个旧故障注入对 JSON 中的公开 representation 名称匹配，现继续保留公开名称，不能误改为私有码。

[SQLite 文档](https://www.sqlite.org/withoutrowid.html)说明增量 BLOB I/O 需要 rowid；当前 WITHOUT ROWID 布局不能直接使用它。没有为追求原地写入增加另一套行存储。事件数组预算和精确长度描述每次变化的新事件，本轮不添加缺少稳定身份依据的事件缓存，保留最终验证。

## 验证与测量

最终 15 步 `checks-third`：**2,350 passed、0 failed、1 项既有 ignored**；fmt、release/all-target/all-feature Clippy（`-D warnings`）通过。另加 **8 项互不重复的 artifact lineage 测试**，合计 2,358 项通过。主检查包含 SQLite 640、runtime client 615、72 项真实进程死亡、151 项 durable integration 和 62 项 process integration。四项新回归均实际通过；966 个 src/tests/Cargo 输入在检查前后、额外检查后及随后干净 O3 构建中一致。早期 Arc 键调用点的编译错误和上述失败测试保留。

正确性使用 release 依赖/root O0/all features；性能单独干净构建 O3/default features。本任务编译/检查在正式产品之前结束。正式协议是六场景 × 两模式 × 六平衡顺序配对：**144 产品、72 对、1,692 请求对照**；CPU 2、8，各格旧→新/新→旧各三对。13 个测量/审计脚本预先冻结并核对同一清单。控制线程 affinity、拓扑与继承限制被记录，不宣称机器独占或持续观测每个子线程。

**Pi 和 DeepSeek 未重跑。** 固定引用仍为 Pi Durable `90d863f7c1183b965f099855715d6e88c7b8a52c` 和 DeepSeek `687f282e8625c6b8d11ff58211a90524be2e4317` 的 60/12 个历史产品。历史批次 CPU 未限制，原值单列，不作为本轮的新倍数分母或追平证据。DeepSeek SDK idle 不等于最终 flush/fsync，完整进程写入不可得；Rust/Pi 核心等价性排除工具定义，基准不含 AppServer transport。

## 正式结果

百分比为六个配对变化的中位数，不是批次中位数之比，也不将所有 turn 混池。请求 CPU 为测量窗口；完整 CPU/写入截至 cleanup。CPU 原计数 100 Hz，端点粒度 10 ms。

| 模式 | 历史轮数 | median wall | mean wall | 请求 CPU | 完整 CPU | 完整写入 |
|---|---:|---:|---:|---:|---:|---:|
| 无实时订阅 | 8 | -1.299% | -0.376% | +0.000% | +0.575% | -0.599% |
| 无实时订阅 | 64 | -1.999% | -0.627% | -1.667% | -0.159% | -0.852% |
| 实时订阅 | 8 | -4.229% | -3.282% | -1.515% | +0.000% | -0.611% |
| 实时订阅 | 64 | +3.277% | +2.395% | +4.206% | +1.233% | -0.856% |

所有场景、绝对值、范围、逐对反例和不可用比例见[完整表格](2026-11-02-content-append-tables.md)。
逐对 median wall：**35 降、0 同、37 升**。
完整 CPU：**14 降、34 同、24 升**。
完整写入：**30 降、41 同、1 升**。
采样 RSS：**22 降、1 同、49 升**。

no-live-subscriber stream：wall **+0.823%**，请求 CPU **+13.846%**，完整 CPU **+6.719%**，完整写入 **+0.000%**。
live-subscriber stream：wall **-0.989%**，请求 CPU **-4.167%**，完整 CPU **+0.000%**，完整写入 **+0.000%**。

上述为合并实现的直接测量，不能分配每个改动的精确因果比例，也不支持由少数格宣称全场景更快或追平 Pi。所有上升和长尾均保留，未重跑筛除不利格。

## 写入诊断与回退重现

四个 columns 与四个 WAL 产品独立审计。以下均为 32 个热轮次的实际值，页面归属来自端点快照，不能当作每次写入时的严格因果归属。

| 历史轮数 | 主库提交 before→after | WAL frames before→after | frontier 列值 bytes before→after | 其中 representation bytes | 其中 source_kind bytes |
|---|---:|---:|---:|---:|---:|
| 8 | 160→160 | 5714→5665 | 47030→37430 | 7136→480 | 7936→4992 |
| 64 | 160→160 | 6446→6389 | 47296→37696 | 7136→480 | 7936→4992 |

逻辑列字节、WAL 帧、进程 write_bytes 和设备流量不互换；main-only columns 不包括连接本地 TEMP。缩短固定键会直接减少对应列值字节，但 WAL 和完整进程写入是否下降以各自测量为准。

代码修改前，另运行 **24 个流式重现产品 / 12 对**，对照更早的 2184461 与上一版 a664c5a，均为 schema 87。独立审计通过，和本轮正式矩阵分开，不能混池或将百分比跨批次相减。该重现属于探索性调查，未预注册在最终 gate 下。实时订阅完整 CPU 中位为 −8.167%，无订阅为 +9.416%，与此前回退方向不一致；不据此抹除旧结果或认定唯一原因。详见[全部重现指标](2026-11-02-stream-replication.md)。最早一次该补充审计遗漏数学模块初始化，失败报告保留，修正后复算同一数据，没有重跑产品。

## 原始证据

原 WAL 审计漏识别了一条带 strace resume 对齐空格的完整帧头，导致帧头/载荷计数不一致。原 13 个冻结脚本及失败审计保留；新增 corrected 解析器只放宽系统调用数字/返回值外的空白，并使用新 summary 文件名，对同一批 trace 重算，没有重跑产品。修正版属于测量后审计修正，精确 diff/哈希记录于 wal-parser-correction.json，pack/verifier 强制核对这一变换。正式性能矩阵和统计未改变。

[归档与复验说明](evidence/content-runtime-20261102/README.md)：**1,254 个 payload 文件**，压缩 **11,264,364 bytes**，SHA256 `0d5aa082ee331274948a0946cb31188e593e9b0d5a08850151861d036914f741`。本轮共 **176 个新 Rust 产品**：正式 144、探索重现 24、columns/WAL 8；阶段分开审计。

[独立 portable 复验](evidence/content-runtime-20261102/portable-verification.json)通过安全解压、逐文件哈希、原始 formal/重现/diagnostic 算术、完整表格一致性和复算后不可变检查。不执行产品、编译器或历史引擎。源码补丁、构建日志、全部失败检查及不利数据保留，可执行文件与依赖缓存排除。

没有运行全仓库 CI/browser E2E，也未合并 main；PR 保持 draft，原有 main 冲突仍在。此前报告、原始归档和不利批次均未覆盖。
