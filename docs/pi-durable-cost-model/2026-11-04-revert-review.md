# PR #485：性能改动保留／撤回评审清单

评审日期：2026-11-04。审阅快照：[baf41cebf](https://github.com/Caismis/rustX/commit/baf41cebf86bda56523277980bf7fdcfb0d01af7)，其最新生产源码为 [e7289ac18](https://github.com/Caismis/rustX/commit/e7289ac18f16d3bdde6c822e4260c0496791f28a)。本文供独立评审者决定保留、简化或撤回，**不执行任何性能代码撤回，不重新运行 rustX、Pi 或 DeepSeek 基准**。依据为 PR 中保存的配对结果、诊断报告、提交差异及当前源码；没有把历史结果当成当前版本的反向实验。

**建议先评审 A1（统计增量历史）、A2（多层私有编码）、A3/A4（metadata／Attempt 准备缓存）、A5（语句缓存容量）。B1 的独立捕获认证是另一项应明确确认的产品合同。** A6–A10 的收益证据较弱，但其中有简单且合理的去重，不应只因幅度小就撤回。B 类有明确收益，默认保留，列出的是实际代价；C 类不是当前待撤项。

## 证据如何使用

- 百分比沿用各报告的配对统计；负数表示该指标减少。轮次中位耗时、轮次均值、请求 CPU、完整周期 CPU、逻辑列字节、WAL 帧、进程 write_bytes、数据库占用和 PSS 不能互换。
- 多项改动一起测得的结果，不能分摊为某个子补丁的独立效果；某格退化也不等于已找到原因。没有把不同批次收益相乘、相减，或据其推算撤回后的速度。
- 后续复测未稳定复现的退化仍列出，但不称为当前必现；已经撤销的试验及已修复的缺陷单列。
- 当前大量实现相互依赖。下文提交是追溯入口，**不是可以直接 git revert 的操作清单**。尤其不能恢复旧的存储信任关系、规范化缺陷或错误顺序。
- 私有持久化格式已有多轮变更，当前 SQLite schema 为 94。撤回格式设计须重新决定当前 schema／开发数据库处理，遵守仓库不保留兼容层、不加迁移的原则；不能承诺旧提交直接适配现有库。本次不修改数据库。

## A．低收益、负向结果或收益归因不足，优先做保留价值审查

| ID／顺序 | 主题与主要提交 | 已有证据 | 初步判断 |
| --- | --- | --- | --- |
| A1／高 | Statistics 贡献＋稀疏 checkpoint：[ecf0efa00](https://github.com/Caismis/rustX/commit/ecf0efa00dd01dc584c5cb31960d972cfe613b51)、[c9d5f73e5](https://github.com/Caismis/rustX/commit/c9d5f73e59abc2601d9f57365157580a635aca10) | 统计逻辑字节约 −67%，总逻辑字节约 −3.6%；WAL 仅约 −0.22%，完整周期进程写入仅 −0.11%…−0.33%；15 条后缀历史读取明显更贵 | 最值得审查的结构性复杂度；尚无整体加速证据 |
| A2／高 | 紧凑观察记录、MessagePack／tuple、导航页前缀压缩及快速准入：[40f0b1d22](https://github.com/Caismis/rustX/commit/40f0b1d22a9ea320359628709399cc7c095742bf)、[978ff5554](https://github.com/Caismis/rustX/commit/978ff5554f17b2214872addd6090ff4278143378)、[11fb672eb](https://github.com/Caismis/rustX/commit/11fb672eb2513195e31a4d2b7ba9726af0289ee6)，后续多次扩展 | 早期实际写入约 −2.17%…−3.65%，但 live 长历史耗时 +31.51%、窗口 CPU +22.22%；另一轮 Statistics/Execution 列字节约 −27%，WAL 几乎不动 | 按 codec／页压缩／准入 fast path 拆开审查，不能凭列压缩幅度批准整层复杂度 |
| A3／高 | metadata 排序、合并及预算证书缓存：[57304c804](https://github.com/Caismis/rustX/commit/57304c8047fc49894aa502dbdda747c6ad6180f3)、[43f56cfcc](https://github.com/Caismis/rustX/commit/43f56cfccc7d0b8b71dfb3ff0278ab0d2da06d22)、[a664c5a96](https://github.com/Caismis/rustX/commit/a664c5a96546c3e49c5638c3c959b08df3c77fec) | 重复工作整组仅写入 −0.73%…−2.19%，12 格延迟 8 格回退；后续 metadata 轮无订阅近乎持平，live 热轮有改善，live stream 仍有反向结果 | 优先评审缓存层，保留简单遍历合并与不可变数据共享 |
| A4／高 | Attempt 分项 owner／prepared capture／admission replay：[11fb672eb](https://github.com/Caismis/rustX/commit/11fb672eb2513195e31a4d2b7ba9726af0289ee6)、[e9df19d2d](https://github.com/Caismis/rustX/commit/e9df19d2db38961343d4f05aa7d9c601fecc5968)、[2184461af](https://github.com/Caismis/rustX/commit/2184461af208af8480a53497a91aabb059c44e16) | 218 轮长历史耗时 +5.11%／+4.99%，窗口 CPU +5.06%／+8.00%；后续消融未稳定复现旧回归，也未证明去掉某层总能更快 | 缓存状态与收益需要重新对齐；现有证据不足以直接定罪或整组撤回 |
| A5／高 | 固定 SQL plan 复用与每连接 256 条缓存：[b87ae908b](https://github.com/Caismis/rustX/commit/b87ae908b17dc9b63c2e6993a0c81d10c0960177)、[91a00da4d](https://github.com/Caismis/rustX/commit/91a00da4d66ce721b67e36dd35c35b57ee462bd8)、[d1825d0d2](https://github.com/Caismis/rustX/commit/d1825d0d24f9a44c18ac7b1c847cf8fc010a17eb)、[688e7c3e8](https://github.com/Caismis/rustX/commit/688e7c3e85a72ae7d3745aff7419fdcff21c4ec8) | 早期 16 槽轮无一致热轮收益、live stream CPU +7.14%；后期旧诊断每连接约 722 KiB statement 内存、128 连接约 90 MiB，约 210 个 plan | 重点审查容量及真正需要缓存的语句，不等同于取消全部 prepare_cached |
| A6／中 | State／普通 Request 字段预览延后：[b5840bd90](https://github.com/Caismis/rustX/commit/b5840bd903d2508e693e1a4f80c3dc6173c9e29b)、[d22ce66c5](https://github.com/Caismis/rustX/commit/d22ce66c5a1cda895cac382ce2c350f345933876) | 早期 State preview 写入只减约 0.4%…1.2%，CPU／内存混合；检查时增加原始字节及索引工作 | 评审冷检查代价、raw 依赖和实现复杂度；并非免费删除工作 |
| A7／中 | 常见小事件 Source 延后：[09e388f85](https://github.com/Caismis/rustX/commit/09e388f85c3b1605e862fc840ccc7a7e89a75e1a)；边界修复见 [ecf0efa00](https://github.com/Caismis/rustX/commit/ecf0efa00dd01dc584c5cb31960d972cfe613b51) | Source 表示小约 57.7%，总逻辑数据只少约 1.1%，WAL 基本相同；live 热轮改善约 4.2%…6.8%，无订阅收益弱；首次检查成本后移 | 局部有收益、架构合理；只可按实际收益与检查频率评审 |
| A8／中 | 内容私有短码、BLOB 尾块合并、共享 read-cut 键：[274d5d47e](https://github.com/Caismis/rustX/commit/274d5d47e6e791cd9093aa40b64965def6d178fc) | 热轮完整进程写入约 −0.60%…−0.86%；live 长历史耗时 +3.28%、窗口 CPU +4.21%；无订阅 stream CPU 反向 | 简单短码与真正去重复读写可保留；没有独立证据证明每个子项的净收益 |
| A9／低 | 仅 observation_models 改 WITHOUT ROWID：[e44a6e545](https://github.com/Caismis/rustX/commit/e44a6e545895484638807e96f8fc3531be99728f)、收窄 [1039cc44d](https://github.com/Caismis/rustX/commit/1039cc44deff7e13b8a8275ab12e8b2b09cb7ad2) | WAL 诊断约 −0.51%／−0.19%；普通进程写入三格小降、一格 +0.084%；删除一棵真实 B-tree | 收益小，但实现也简单；默认保留，不是主要负担 |
| A10／低 | 新 leaf 的 template metrics 合并计算：[989d74d12](https://github.com/Caismis/rustX/commit/989d74d12d211a38eee5ad1f07ec18c0251a0de5) | 最早隔离短／长热轮 wall +1.12%／+1.64%，CPU +1.61%／+1.56%，写入几乎不变；其他场景混合 | 未证实整体收益；当前代码只是有界局部去重，未必值得撤回 |

### A1．统计稀疏历史：收益最容易被高估的一项

当前入口：[history.rs](../../src/durable/sqlite/observation_statistics/history.rs)、[contribution.rs](../../src/durable/sqlite/observation_statistics/contribution.rs)、[统计 owner](../../src/durable/sqlite/observation_statistics/owner.rs)。证据：[统计报告](2026-11-03-statistics-history.md)、[完整表](2026-11-03-statistics-history-tables.md)、[后续长历史复测](2026-11-03-request-hotpath.md)。

- 每 32 热轮完整 tuple 192→32，仍有 192 份贡献；WAL 5,669→5,656／6,390→6,376，提交仍 160→160。不能称为“写入减少 67%”。
- 64-history 普通历史读取，15 条后缀首次 53.903→221.454 µs，重复 16.799→47.880 µs；更深历史产品也更慢。终态 checkpoint 的直接读取不会重放 15 条，不能把全部热轮成本都归给后缀重放。
- 该轮 no-live／live 长历史耗时 +5.395%／+2.656%；后续独立 repeat 未稳定复现同一方向，并修复了只读 rollback 引起的 proof 失效。旧回归应保留，不能当成当前必现。
- 最小审查单元是“贡献持久化、checkpoint 定位及后缀读取”，与“提交拥有的内存 fold／统计派生权威”分开。即使撤回稀疏存储，也不应重新让外部合法改写的总数支配后续累计统计。
- 评审问题：任意历史统计 cut 的紧凑存储是否值得这些 codec、摘要、缺口传播、重放和事务暂存状态？能否以更简单的完整历史物化换回更便宜读取？统计独立失败、时间推进、旧 cut 和 rollback 规则必须保留。

### A2．私有编码：压缩收益与双重准入成本分开判断

当前入口：[observation_codec.rs](../../src/durable/sqlite/observation_codec.rs)、[request record](../../src/durable/sqlite/observation_request/composed/record.rs)、[page_codec.rs](../../src/durable/sqlite/observation_request/composed/page_codec.rs)。证据：[紧凑布局](2026-10-29-compact-write-layout.md)、[重复工作](2026-10-30-repeated-work-final.md)、[共享域终版](2026-10-30-shared-domains-final.md)。

- 当前不仅做二进制 encode/decode，还须保留原 JSON 公开准入、非有限值规范化、结构预扫描、EOF、解压前后页界限及未知字段计长。存得小不代表解码／校验更便宜。
- 早期 +31.506% 耗时／+22.222% 窗口 CPU 来自多项改动组合，不是 page prefix 或 MessagePack 单项因果证据；后续源码也已变化。
- 共享域轮 Statistics＋Execution 列字节约少 27%，全表逻辑字节只少约 2.8%…2.9%，WAL 5,748→5,742／6,449→6,453；那一轮还有四项其他改动，不能把 2%…5% 热轮改善单独归给二进制。
- 最小单元分别为事件 evidence codec、统计／执行 tuple codec、request 导航页 prefix codec、公开长度 fast path。A1 的统计贡献依赖当前 codec；当前格式上做撤回还须处理 schema 及精确准入，不能直接改回一个旧 JSON reader。
- 评审问题：哪一种格式实际省页／减少工作，哪一种只省少量列值却引入更多校验分支？退回简单编码时保留原始 opaque JSON、数值／顺序及独立错误域。

### A3．metadata 缓存：区分共享正文与缓存计算结果

当前入口：[PreparedArray／ArrayCache／MetadataArrays](../../src/runtime_client/checkpoint/metadata_fragment.rs)、[prepared_capture.rs](../../src/runtime_client/projection/prepared_capture.rs)、[read_cut.rs](../../src/runtime_client/checkpoint/read_cut.rs)。证据：[重复工作终版](2026-10-30-repeated-work-final.md)、[metadata 报告](2026-11-02-shared-metadata.md)、[消融](2026-11-02-metadata-ablations.md)。

- 首轮整组 60 对写入均下降，但只约 0.73%…2.19%；延迟 24 降、36 升，live 短历史完整 CPU +8.511%，不能称为整体计算减少。
- 后续 metadata 轮 live 短／长热轮耗时 −4.681%／−1.731%，no-live 近乎不变；live stream 耗时 +1.317%、完整 CPU +4.026%。另一次 stream replication 方向又改变，未形成稳定单项因果归因。
- 缓存依然要检查 owner 顺序、比较数组、锁住最新记录；frontier／cut 频繁变化时会 miss。当前有排序 permutation、长度／预算证书、evidence merge、prefix binding 等不同层。
- 建议审查排序／合并结果缓存及重复证书，不把已有 Arc／持久映射正文共享一起删除。本轮最新对象诊断中 binding／frontier 唯一数几乎不变，也不支持“metadata 这一子项省了大量正文内存”。
- 评审问题：每一层有何独立命中与准备成本依据？更简单的有序构造能否取代状态缓存？保留来源首项优先、重复项顺序、budget／eviction、当前来源重解析。

### A4．Attempt 准备：反例存在，撤回更快尚未证实

当前入口：[prepared_attempt.rs](../../src/runtime_client/projection/prepared_attempt.rs)、[SharedAttempt](../../src/runtime_client/projection/shared_attempt.rs)。证据：[Attempt 轮](2026-11-02-incremental-attempt.md)、[独立消融](2026-11-02-metadata-ablations.md)。

- 218 轮 64-history 耗时 +5.109%／+4.987%，请求 CPU +5.057%／+8.001%；同时只有约 0.34%…0.61% 完整写入下降。该提交还含索引缩窄等其他改动。
- 消融中，把 deep detach 恢复成深复制在 no-live 六对全部更慢，耗时中位 +1.498%；改回 eager Attempt 为 +1.801%／+0.030%，没有稳定胜出。旧版本对照复现仅 +0.707%／+0.448%，旧约 5% 方向未稳定重现。
- 可审查分项 owner、PreparedItem、fit／Capture 重放三层的必要性，但不能把“长历史曾退化”直接翻译为“Arc／lazy 一定有害”。
- 必须保留当前共享预算、非恒等规范化、signed zero／JSON 顺序、Tool 当前事实与变化行 detach。218 的索引尾部 sequence 去重是另一单元，撤回缓存不应顺手把它恢复。

### A5．SQL cache：审查的是保留工作集，不是取消所有 SQL 复用

当前入口：[连接初始化](../../src/durable/sqlite.rs)、[idle_cache.rs](../../src/durable/sqlite/idle_cache.rs)、[固定语句](../../src/durable/sqlite/statements.rs)。证据：[早期 16 槽隔离测量](native-statement-reuse-results.md)、[集中优化](2026-10-27-complete-costs.md)、[内存直接计数](2026-11-03-memory-writes.md)、[闲置策略](2026-11-03-idle-retention.md)。

- 容量演进是默认 16→64（91a）→256（688），不是 91a 一步设为 256。早期 16 槽的 mixed 结果不能直接证明当前 256 槽方案有害。
- 688 轮曾观测旧 64 槽下 171 种 SQL shape，整组修改使 prepare 调用约少 75.88%；这是支持保留合理工作集的证据，但不是“256 比其他容量”的独立对照。
- 较早实际诊断每连接约 722 KiB statement allocation，128 连接约 90 MiB、每连接约 210 个 plan。该计数是当时私有探针，不是最新 PSS 分区或当前必须能释放的精确字节。
- 当前 120 秒回收可清空 idle plan，活跃连接仍保留 256 槽。最小审查单元为容量／哪些固定语句值得留在 cache；不撤回事务 proof，不把 plan cache 当作结果／BLOB 缓存。
- 评审问题：128 会话只有少数活跃时，当前策略的内存与重新 prepare 成本是否合适？不要在没有工作集证据时直接退到 16／32 槽，也不要把另一个已撤销的 876 试验混进来。

### A6–A10．次级清理候选

**A6：预览延后。** 当前 [state_preview.rs](../../src/durable/sqlite/observation_request/state_preview.rs) 和 [request read](../../src/durable/sqlite/observation_request/composed/read.rs) 仍从 geometry／raw 派生部分预览。早期隔离普通写入只少约 0.4%…1.2%，检查增加 SQL／字节工作；后来的 d22 还将普通 Request Fact.header 镜像取消，预览因此依赖对应 raw page。见[早期决策矩阵](decision-matrix.md)、[Pi-flow 合同](2026-10-27-pi-flow.md)。评审是否值得保留这一职责划分及冷读／故障依赖；若调整，不能重新提前构造整请求历史。两次改变不是同一个补丁，旧 isolated 数字不能当作 d22 子项收益。

**A7：小事件 Source。** 当前 [inline.rs](../../src/durable/sqlite/observation_event/inline.rs) 仅允许九类闭合事件、native 长度不超过 16,320 字节延后。见[边界报告](2026-11-03-observation-boundary.md)、[后续解释](2026-11-03-statistics-history.md)。旧批 no-live stream wall +1.505%、完整 CPU +9.583%，后续八请求窗口未稳定重现；首次检查约多 1.8 ms，后来确认存在进程共享 schema／plan 首次初始化，不能按事件数乘这个数字。重复检查仍解码和重建，并非命中事件 Source 缓存。此项不应以“写入 −57.7%”作为保留理由，但当前按需路线有合理边界。撤回时不能恢复已修复的 16 KiB 临时索引台阶，且聚合借用 BLOB／转移缓冲区是独立的简单改进。

**A8：短码和尾块。** 当前 [content.rs](../../src/durable/sqlite/content.rs)、[公开 representation](../../src/runtime_client/content.rs)。见[内容轮](2026-11-02-content-append.md)。固定短码没有字典或新配置，尾块合并用严格 BLOB 函数，代码未必比旧多步读写复杂；但整组只省不足 1% 完整写入，stream 本身完整写入中位数未降。优先审查短码／读写合并各自收益，不把波动都归于其中之一；任何撤回不得恢复曾破坏 UTF-16 奇数字节的 SQL TEXT 拼接。

**A9：仅 models 聚簇。** 当前 [模型表与 schema gate](../../src/durable/sqlite.rs)，见[资源容量报告](2026-11-03-resources-capacity.md)。这是小行复合主键的一棵树替代 rowid＋PK 树，不含 lifecycle 聚簇。可以作为低价值格式特化评审，但它没有新增 cache／失效状态，撤回可能反而多维护一棵树，默认保留。

**A10：template metrics。** 当前 [leaf_geometry_reusing](../../src/durable/sqlite/observation_request/composed/nodes.rs) 仍只暂存最多 80 个宽度基值、每模板计算一次，没有全局 cache。见[隔离测量](template-metrics.md)。早期短请求略退化，不足以证明当前有害；最新方法已经与 CapturedTemplate、WidthTable 合并，旧 helper 已非原样。它更多是简单去重而非复杂性能架构，优先级最低。

## B．收益明确，但有代价／合同变化，应单独确认

| ID | 改动／追溯 | 确认的收益与代价 | 评审建议 |
| --- | --- | --- | --- |
| B1 | 独立原始捕获认证移除：[5e37b9240](https://github.com/Caismis/rustX/commit/5e37b9240290723483ac00855cee7eee58760c81) | final-context 短热轮 CPU 约 −6.3%，写入仅约 −0.2%；一致改写可选 Source 图后可通过当前图校验 | 优先确认产品是否需要独立 original-capture 证明；这是完整性合同，不只是崩溃持久性 |
| B2 | 原子最终结算／Pi 式连接内 readiness：[ce8f7270e](https://github.com/Caismis/rustX/commit/ce8f7270ef65ef279f9c5d6c73135b420074dab0)、[d22ce66c5](https://github.com/Caismis/rustX/commit/d22ce66c5a1cda895cac382ce2c350f345933876) | 早期 atomic 单独短请求产品全周期进程写入约 −18.7%…−20.1%，但取消 wall +4.7%；后续部分安装进度改为进程内 TEMP 状态，重开不恢复旧进程安装／通知完成状态 | 有明确收益且符合此前 Pi 持久性目标；恢复合同应单独签认，不建议整组倒退 |
| B3 | 100 ms partial 合并：[a3c116fe9](https://github.com/Caismis/rustX/commit/a3c116fe91d7689d86c5009c7b563289d3cd34cc) | burst 大输出 wall／CPU 约 −73%／−68%，全周期进程写入 −66%；短长历史 wall／CPU +2.6%／+1.7%，tool CPU +9.1%，cancel wall +3.6%；显示更稀、未提交尾部更大 | 按交互及恢复粒度评审，不能只看整轮耗时；默认保留合并机制 |
| B4 | 2 KiB SQLite 页＋block-header rowid：[641f78121](https://github.com/Caismis/rustX/commit/641f78121ca731f0f5c01cc453586cc58fffa19e)、[b0ac8ed4f](https://github.com/Caismis/rustX/commit/b0ac8ed4f06ef15c6da52438eee06a4e00fd7737) | 热轮实际进程写入约 −25%…−28%；每 32 轮主库已用页增长反而 +6.4%…+6.9%，WAL 帧数可增加 | 明确以写放大换空间密度；默认保留，勿把已处理的 +71.2% native wall 直接归罪于页尺寸 |
| B5 | 30 s page／120 s plan 回收＋批量 GNU trim：[f2e1273d2](https://github.com/Caismis/rustX/commit/f2e1273d2fd297fecdd0485d9edaa1a97520e73e)、[d50125842](https://github.com/Caismis/rustX/commit/d50125842f45721e831d02474bf77bf2b7c62bc5) | 释放 live cache，较早普通 130 s 混合 PSS 772.57→698.40 MiB；冷恢复 10.08→13.14 ms、CPU +29.59%；单次受控 trim 释放约 45.78 MiB、控制线程约 12.71 ms CPU | 这是可量化的 idle 内存／恢复延迟交换；独立审查阈值与全局 trim，不以热窗回退证明其因果 |
| B6 | 64 KiB 首块内联 frontier：[ec036e46a](https://github.com/Caismis/rustX/commit/ec036e46a32122f69c6840e4288c49ca7783444c)、[ef63fb2cd](https://github.com/Caismis/rustX/commit/ef63fb2cd35d18fb0223d0729d2ab98191f2838b) | 热轮进程写入约 −6%…−7.4%，内容两表合计 WAL 页减少；frontier 更宽，跨 inline／overflow 的正文窗口从一条 SQL 变两条 | 默认保留写入收益；布局／范围读开销可单独评审，别连同正文／proof 共享一起撤 |
| B7 | 最新共享行与冻结 Vec 容量：[f881e0be3](https://github.com/Caismis/rustX/commit/f881e0be384efa8ddb00cadd532ea2d60e7fb472)、[e7289ac18](https://github.com/Caismis/rustX/commit/e7289ac18f16d3bdde6c822e4260c0496791f28a) | 128×8 PSS 584.89→480.75 MiB，聊天增量 −24.73%；单并发 wall +1.89%、CPU +2.94%，width-8 P99 +10.20%；单个暖恢复对 9.01→10.04 ms | 默认保留约 104 MiB 收益；性能代价公开。metadata 子项唯一对象数几乎未变，别把收益都归给 metadata |
| B8 | observer-demand／Trace 延后：[b42faa94c](https://github.com/Caismis/rustX/commit/b42faa94cb0d579812580f0342cf7e0cd3d90b63)、[473ad1a9e](https://github.com/Caismis/rustX/commit/473ad1a9e6b0263afe057553dd576f952aa35101)及恢复修复 | 早期 no-live 热轮 wall 少约 36%…39%、CPU 少约 49%…52%；原 live cancel +15.8%／+25%，严格输入控制未复现旧点估计 | 核心按需读取收益明确；单独审查 Trace 调度复杂度，保留 frozen-cut／恢复正确性修复 |
| B9 | QPSG／紧凑临时导航：[15e7e7125](https://github.com/Caismis/rustX/commit/15e7e7125dc34f54a245ea06e95f517d67c9ea53)、[97dfdca84](https://github.com/Caismis/rustX/commit/97dfdca84335f6f5350c78356ab04ace1d1c11fe)，有界 byte residency [e1823321f](https://github.com/Caismis/rustX/commit/e1823321fa0dce1e3969f571e4eb61bb85aa2924)、[c34de111f](https://github.com/Caismis/rustX/commit/c34de111f5da991e5b92db40a9391d60842b8c49) | QPSG 早期短热轮约 −11% wall，但 large CPU +14.3% 且只有 6–8 ticks；紧凑导航短热轮约 −3%…−5%，区间跨 1；16 KiB residency 明确减少临时文件／read | 没有当前有害证明。简单 bounded 实现默认保留；参数／数据分布适用性与驻留预算可独立审查 |

### B1 的现状必须准确表述

[当前 request_child_reference_tests.rs](../../src/durable/sqlite/request_child_reference_tests.rs) 的 t08_t12_t19_page_integrity_is_not_independent_capture_authentication 明确展示：同步改写可选 graph 和 checksum 后，历史字段图可以通过当前校验。原生冻结请求及 canonical history 是独立权威，不能据此被改写；损坏、无效类型和不一致摘要仍拒绝。

[inspection-integrity.md](inspection-integrity.md) 和 [authentication-leave-out.md](authentication-leave-out.md) 记载该取舍及约 6.3% 短请求 CPU 的 final-context 收益。历史 [3ef41465d](https://github.com/Caismis/rustX/commit/3ef41465d951287b1e37c82af70fd11f631cdd29)、[49ee1d158](https://github.com/Caismis/rustX/commit/49ee1d15899058feb0ae2f80c1911355a78545ba)、[e17893c63](https://github.com/Caismis/rustX/commit/e17893c63b18025ccd56ff7d8e41ec77bc494ea6) 保存的是 RawValue／认证恢复的证据或诊断 patch，**不是把当前生产认证已经恢复的证明**。当前仍无独立 original-capture witness。

评审者应决定需要的是“当前内容寻址图自洽＋原生请求独立权威”，还是“可选图也必须对原始捕获独立认证”。如果决定恢复后者，必须保留 RawValue 原始数字、顺序、词法及后续请求共享修复，不能整笔回退 5e37 把它们一起撤掉。此前允许 Pi 级崩溃持久性，不应在评审中直接等同于这两种认证合同没有区别。

### B 类的归因边界

- B2 与 B3 的早期单独试验、最终上下文 leave-out 和后续 Pi-flow 是不同批次。独立中间崩溃切点、旧进程 notification/install 进度与 live 部分尾部是不同保证，不能合并为“fsync 更少”。
- B3 当前策略代码：[coalescer.rs](../../src/publication/coalescer.rs)。100 ms 是最老缓冲 deadline 的合并阈值，最终还要提交和调度，不是端到端可见性 SLA。
- B4 原始 native 单并发 +71.235%／CPU +13.177% 是真实历史反例；随后 [e1d4a40dc](https://github.com/Caismis/rustX/commit/e1d4a40dc7876d79ca0b3240064b560659b7d593) 用 TCP_NODELAY 与目录提交重复准备的独立证据修复主要可避免成本。没有证据把原全部 71.2% 唯一归于页尺寸，也没有证明页尺寸完全无代价。
- B5 的 trim 在一次实际完成回收的批次后执行，见 [manager](../../src/local_runtime/session_runtime_manager.rs)。旧 width-4 热窗不满足 30 秒条件，不能用该热窗 +4.13% CPU 给 trim 定因果。glibc PSS、SQLite API 近似分类及替换分配器 live allocation 分开记录。
- B6 原批 live 短历史 wall +66.937%／窗口 CPU +47.826% 不能隐去；[运行顺序及核心限制调查](2026-10-30-inline-content.md) 在固定双核下不复现这一幅度且方向随先后顺序变化，未确立某个子补丁的因果。实际写入收益则有独立 WAL／表行依据。
- B7 的四并发固定／轮换 CPU 为 −2.89%／−0.96%，仍有单个轮换对 +3.28%；所有反向原值见[最新报告](2026-11-04-observation-ownership.md)。这不是全场景零成本，也不是可以据单个 P99 撤销约 104 MiB 收益的结论。
- B8 见[原始矩阵](observer-demand-results.md)、[严格取消控制](observer-demand-cancellation-v3.md)、[Trace 阶段](live-trace-demand.md)。不把旧 normalized-input 取消回归当成已确认的当前点估计；后续精确 cut、重连、Source pin 和 terminal repair 是正确性修复，不能随可选 eager/lazy 策略直接移除。
- B9 见[query-plan](query-plan-results.md)、[compact-navigation](compact-navigation-results.md)、[16 KiB residency](resident-bound.md)。三对和 CPU 粒度限制已保留；源码仍启用 QPSG。减少 /tmp 文件也不等于减少 SSD 写入，不能无条件把早期 4 KiB residency 的反向结果套在后来 16 KiB 实现。

## C．已经撤销／修正，不能再次作为当前待撤方案

| 项目 | 历史问题 | 当前状态与证据 |
| --- | --- | --- |
| 额外 native read SQL cache 试验 | [8760f5709](https://github.com/Caismis/rustX/commit/8760f570999a4693f59db5c446804afce34b74c8) 的部分短请求更快，但多个 cold／large／tool 指标退化 | 已由 [caaab5f2b](https://github.com/Caismis/rustX/commit/caaab5f2b6d6c3ec9d0f19a35249c5fe70e367a8) 撤销，见 [性能报告](2026-10-27-performance.md)；后来某些调用再用 prepare_cached，不等于原试验整体仍原样存在 |
| 早期 request scalar 延后试验 | [013d57990](https://github.com/Caismis/rustX/commit/013d57990efecb0d6f868914fc870654cd124ca3) 等降低写入，但 dense inspection 一致变慢，部分热轮也退化 | 已由 [6e1ca189a](https://github.com/Caismis/rustX/commit/6e1ca189abfa9879fe87d2e4ecd053530434a491) 撤销，见 [continuation](2026-10-27-continuation.md)；当前同名 scalar.rs 是后续请求配置／prefix 捕获路径，不能按文件名删除 |
| lifecycle_state 与 models 一起聚簇 | [e44a6e545](https://github.com/Caismis/rustX/commit/e44a6e545895484638807e96f8fc3531be99728f) 长历史写入 +0.465%／+0.375%，lifecycle 归属页增多 | [1039cc44d](https://github.com/Caismis/rustX/commit/1039cc44deff7e13b8a8275ab12e8b2b09cb7ad2) 已恢复 lifecycle rowid，只留 models；见[资源报告](2026-11-03-resources-capacity.md) |
| 小页下 block header 的聚簇 BLOB 溢出 | 初试主库占用增约 14%，medium BLOB 出 overflow | [b0ac8ed4f](https://github.com/Caismis/rustX/commit/b0ac8ed4f06ef15c6da52438eee06a4e00fd7737) 改回普通 rowid header，最终占用代价约 +6.4%…+6.9%；不把初试布局视为当前状态 |
| live capture cache miss 重复 encode／parse | [11fb672eb](https://github.com/Caismis/rustX/commit/11fb672eb2513195e31a4d2b7ba9726af0289ee6) 首批 stream 回退，增长 miss 会重复准备 | [e9df19d2d](https://github.com/Caismis/rustX/commit/e9df19d2db38961343d4f05aa7d9c601fecc5968) 已合并单一路径，见[终版](2026-10-30-shared-domains-final.md)；不能只用首批 +14.091% 请求 CPU 评价终版 |
| 16 KiB deferred Source 的临时索引台阶 | 首版 native→structured 长度增长跨阈值，重复读取约 420–432 µs | [ecf0efa00](https://github.com/Caismis/rustX/commit/ecf0efa00dd01dc584c5cb31960d972cfe613b51) 已将 native 准入收紧到 16,320，当前边界有回归；见[统计报告](2026-11-03-statistics-history.md) |
| Source schema／plan 首次约 2 ms | 首次检查较慢 | 后续定向分辨了共享首次初始化；它不是每事件一次，也不是命中事件 Source cache，不能据此估算 128 倍成本 |
| BLOB 尾块 SQL TEXT 拼接 | 对 UTF-16 奇数字节不保真 | 同一 [274d5d47e](https://github.com/Caismis/rustX/commit/274d5d47e6e791cd9093aa40b64965def6d178fc) 开发过程中已删除，当前严格 BLOB 函数；已失败实现不应恢复 |
| 原生单并发 +71.2% | 旧 compact-page 批出现 | 后续 transport／catalog 修复已交付，最新单并发为约 10.34 ms；见[native regression](2026-11-03-native-regression.md)。这是当前回归已不再同幅重现，不是历史样本作废 |

## D．有较强保留依据，不应随混合提交一起倒退

| 改动 | 保留依据／边界 |
| --- | --- |
| [e1d4a40dc](https://github.com/Caismis/rustX/commit/e1d4a40dc7876d79ca0b3240064b560659b7d593) TCP_NODELAY 与 catalog 不可变文档／当前字节复用 | 同一旧 ELF 的 accepted-socket 受控干预有大幅 wall 改善；新产品 native width-1 wall／CPU 约 −50%／−32%，写入约 −6%；[报告](2026-11-03-native-regression.md) |
| [57c7082ea](https://github.com/Caismis/rustX/commit/57c7082ea63b82137c38ccd67e032565d9a1744d) 共享 HTTP/TLS、工具 schema、上游生成 meta 校验与 typed 一次编码 | 128 空会话 PSS 323.61→169.03 MiB，128×8 743.29→585.51 MiB；确有基础占用与重复遍历消除。它是四项组合，不应因一对 CPU +14.63% 整组撤销；[报告](2026-11-04-session-foundations.md) |
| [f881e0be3](https://github.com/Caismis/rustX/commit/f881e0be384efa8ddb00cadd532ea2d60e7fb472)、[e7289ac18](https://github.com/Caismis/rustX/commit/e7289ac18f16d3bdde6c822e4260c0496791f28a) 不可变正文共享及 Vec 容量冻结 | 明确约 104 MiB 普通进程收益、unique row 和实际容量证据，见 B7；保留现有 replay／历史窗口 |
| [d054be089](https://github.com/Caismis/rustX/commit/d054be08981dcc98c53c5ba83641b387f26442d4) scoped event indexes、[57304c804](https://github.com/Caismis/rustX/commit/57304c8047fc49894aa502dbdda747c6ad6180f3) 去除重复 attempt 索引、[2184461af](https://github.com/Caismis/rustX/commit/2184461af208af8480a53497a91aabb059c44e16) 冗余 rowid 尾字段 | 早期 scoped indexes 短／长历史产品全周期进程写入约 −17.4%／−18.9%；其余索引子项幅度小且有性能反例，但都减少真实维护项。保留实际 query/range／故障域合同，不能把原生 kind 与独立 Trace kind 两套直接合并 |
| [688e7c3e8](https://github.com/Caismis/rustX/commit/688e7c3e85a72ae7d3745aff7419fdcff21c4ec8)、[f5cf22a8f](https://github.com/Caismis/rustX/commit/f5cf22a8fbd2b1f0b3abc7fc2082a586f1126e20) typed checkpoint／append facts／同域 UPDATE 合并 | 整组热轮 CPU／wall 与实际 prepare 数有明显下降；其内 A2／A5 仍可单独审查。见[complete costs](2026-10-27-complete-costs.md)，不得把整组收益分摊给单一 codec 或 cache |
| [2afe735f7](https://github.com/Caismis/rustX/commit/2afe735f7c30f6b1451c3232229d7ae58727c911)、[584d581e7](https://github.com/Caismis/rustX/commit/584d581e730d3c03c93e71c4010bdeffa387f888)、[8f6c32ce5](https://github.com/Caismis/rustX/commit/8f6c32ce5a1d7a552f09124806b791316c798914) 完整行一次 INSERT／合并索引／减少重复规范化 | 终版完整写入 60/60 对下降约 4.3%…10.4%；仍有反向场景。见[收尾报告](2026-10-28-final-cost-closure.md) |
| [03ec15b4b](https://github.com/Caismis/rustX/commit/03ec15b4bccccc9459fd1ec7b7fdd1bf4673a617) 节点读取／内容 locator 合并，[fceaf3d82](https://github.com/Caismis/rustX/commit/fceaf3d82548c8fef1844861a1788cbc7ef6f362) 只读 rollback proof 与稳定 request 准备复用 | 前者查询数叶 2→1、枝 4→3，属于少状态的重复消除；后者保持 writer proof、热轮稳定配置完整 Source 编码降为零。速度收益有限，但不宜为“百分比不大”恢复重复操作；[read fusion](2026-11-02-read-fusion.md)、[request hotpath](2026-11-03-request-hotpath.md) |
| [2a8c72166](https://github.com/Caismis/rustX/commit/2a8c721663d0963c74f7499fa635b47c3c83f597) 大 event 变体装箱、[641f78121](https://github.com/Caismis/rustX/commit/641f78121ca731f0f5c01cc453586cc58fffa19e) 共享 capability／小缓冲按需增长 | RuntimeClientEvent 1,168→560 bytes、去掉重复 capability owner、避免小值立即 reserve 64 KiB；不是仅凭一个计时点得出的收益；[内存写入报告](2026-11-03-memory-writes.md) |
| [0f79f8128](https://github.com/Caismis/rustX/commit/0f79f8128d5aea1eca486a7cced822e2a7468adc) checkpoint 充分边界证明、[8f6c32ce5](https://github.com/Caismis/rustX/commit/8f6c32ce5a1d7a552f09124806b791316c798914) 编码计划及 lazy 初始化 | checkpoint-admission 有 live 长历史 wall／CPU 约 −11.4%／−10.8% 的隔离证据；同时保留 stream／cold 反例。复用已证明的边界和编译计划不等于跳过真实输入验证；[隔离结果](checkpoint-admission-results.md)、[收尾](2026-10-28-final-cost-closure.md) |
| [123accb38](https://github.com/Caismis/rustX/commit/123accb38fd8cf1e8f88583c1858d514cb453870)、[459b1b600](https://github.com/Caismis/rustX/commit/459b1b6005bfbf7f0a1a1d52df5a6900b06c56c6)、[ccef6e887](https://github.com/Caismis/rustX/commit/ccef6e8871dda72626e85cb257ec39b32490f300)、[0baac4e2e](https://github.com/Caismis/rustX/commit/0baac4e2e144ea7a9c518798ded4c16123fbb680) 等请求／response／publication owner 合并 | 各轮有成组写入／live 热请求改善；没有当前每个小补丁的独立边际值。评审 owner 层时以现有精确存储 proof 和 B7 内存收益为约束，见[joint](2026-10-27-joint-major-runtime.md)、[owner reuse](2026-10-27-owner-reuse.md)、[merged operations](2026-10-28-merged-operations.md) |

D 类不表示“不可评审”，只是现有数据不足以把它们列为优先撤回对象。尤其那些保持当前事实校验、减少实际读取／写入且没有新全局状态的小改动，不能与维护多层 cache／codec 的设计一概处理。

## 给独立评审者的决策格式

建议每个 ID 给出一种结论：保留；简化一个明确子层；撤回一个明确子层；当前证据不足。请同时注明：

1. 要保留的产品能力／故障或恢复合同，以及该项是纯实现优化还是产品取舍。
2. 具体改动边界和当前依赖；不要只填一个混合提交号。
3. 支持结论的实际指标及测量范围；不把逻辑字节、WAL、设备写入或不同阶段内存混用。
4. 如果要撤回，如何保留旧 cut、精确规范化／budget、当前存储证明、失败隔离、回滚及原始字节正确性；格式改变另列 schema 决策。
5. 尚未做当前反向实验的，明确标为预期／假设，不写成“撤回即可快 N%”。

本清单的建议是先审 A1–A5 和 B1，确认是否值得维护其复杂度／合同；不是重新启动一轮“全部性能优化”，也不预先决定撤回。main v52→v62 迁移仍按已确认范围独立收尾。此次只提交评审文档，生产源码及基准数据保持不变。
