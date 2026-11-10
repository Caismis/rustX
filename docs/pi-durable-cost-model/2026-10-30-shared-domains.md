# 不可变域首轮测量：保留流式回退并修正缓存冷路径

这是 `11fb672eb` 初始候选的完整测量，不代表随后冷路径修正后的终版。初始候选发现 stream 回退后继续修复，最终结果见[后续报告](2026-10-30-shared-domains-final.md)。本轮落实上一轮确认的四项改造：快照按域共享、Attempt 项目准备复用、同次内容读取合并、Statistics/Execution 私有记录二进制化。功能检查通过后，使用固定 CPU 和均衡顺序重新对比新旧 Rust；Pi Durable 和 DeepSeek Harness 没有重跑。

生产提交为 `11fb672eb2513195e31a4d2b7ba9726af0289ee6`，src tree 为 `24397ae655ace6d390c49bc900ab67de60479c64`。fresh Rust baseline 是[上轮生产版](2026-10-30-inline-content.md) `ef63fb2cd35d18fb0223d0729d2ab98191f2838b`，src tree `7add77966c0baf0bdcb4cddd8615d9f1f8029859`，普通 O3/default-feature 二进制 SHA-256 为 `09dd30e83fc942eb6a630f9fe070f60b5c18523243bd07f63a5ad959fbeec6ee`。直接复用该已验证二进制重新运行；未混入旧批次的 Rust 时间样本。

## 已完成的改造

| 改造 | 删除的重复工作 | 保留的语义与成本 |
|---|---|---|
| 不可变快照域 | 内部 `SharedSnapshot` 按域持有 Arc；seed 捕获不再完整深复制 snapshot 各域，也不再序列化遍历 `encoding_equal` 以证明未变 | mutable access 使用 `Arc::make_mut`，旧 seed 不受后续修改影响；真正变化的共享域仍复制。公开 DTO 不变；最终公开输出仍需要构造和编码 |
| Attempt 项目准备 | model/settings、前 32 个 live block 和 Tool 行分别复用准入准备；相同 owner 直接命中，变化的 owner 只比较有限页面 | 当前共享预算逐项重放；预算不足时执行原准入路径。保留 JSON 对象顺序、浮点 signed zero、超限拒绝和旧 cut。Tool 缓存只保存准入结果，当前来源、frontier 和 deferred 装饰仍在缓存外组装；只保留最新 source，不形成历史缓存链 |
| 内容定位与 Tool 原始字节 | canonical descriptor/read 定位由 2 次减为 1 次；长 ID descriptor 由 3 次减为 1 次。四种 block 表示的示例路径由 9 次减为 1 次，含 deferred metadata 时由 11 次减为 1 次。小参数 Tool 分页示例由 8/6 次减为 1 次，删除内部 Base64 编码后再解码 | 只在同一连续事务读取阶段复用 ledger 位置，不跨写入、触发器或 API 缓存。每次仍检查实际 frontier/字节、UTF-8、cut、原始 raw 优先级和生命周期证据。公开内容 API 仍返回 Base64；公开 Tool 分页增加普通只读事务，checkpoint 使用原事务 |
| Statistics/Execution 私有记录 | 复用现有 MessagePack codec，省去私有 JSON tuple 文本与标点；Statistics 的共享判断在一次写入中只计算一次 | 原 16 KiB 逻辑 JSON 准入边界不变，Execution 保留两阶段准入；非有限 timing 保留原 JSON null 语义。读取先校验实际结构与 JSON 长度，再 typed decode 并检查 EOF，包括未知字段和显式 null；不是免校验读取，也不能仅凭编码变小宣称 CPU 更快 |
| 避免无谓域复制 | Tool 更新先只读确定目标所在域，再获取可变引用；安装读取域时保留可精确证明未变的元数据、空正文域和未变 context | 非空、含 JSON 或浮点的域仍执行原安装更新，防止 `PartialEq` 漏掉对象顺序或 signed zero 变化。descriptor→transcript 的更新顺序不变 |

优化后的查询和编码次数由具体回归断言验证；优化前次数按原源码具体路径计数，没有在基线安装计数器重测。它们不直接换算成整体性能百分比。跨首次 inline/overflow 块的内容读取以及独立来源验证仍存在。

SQLite 保持 **WAL/NORMAL**，事务与持久性边界未削弱。schema **85→86**，没有迁移或旧格式兼容解码。三个私有二进制列的物理上限为 49,152 bytes，以容纳相同逻辑 JSON 边界下的二进制表示；逻辑准入仍为 16,384 bytes。旧 schema 明确拒绝打开，符合仓库不保留兼容层的要求。

## 验证与测量口径

最终 **2,323 项测试通过、0 失败、1 项既有忽略**，包括 72 项进程死亡、151 项 durable integration、62 项 process integration；fmt 和全 target/feature Clippy（`-D warnings`）通过。新增 27 项回归，删除已淘汰 `encoding_equal` 路径的一项测试，以不可变 owner、JSON 顺序和 signed-zero 回归替代。成功的 15 步检查绑定 `checks-final`，961 个 src/tests/Cargo 输入前后相同。

正确性使用 release 依赖/root O0/all features；性能使用随后干净构建的普通 O3/default features。初次编译、测试接口和 lint 修复记录保留；只有最终相同源码的成功检查用于证明生产构建。

正式矩阵预先固定为六场景 × 两订阅模式 × 六对，合计 **144 个 Rust 产品、72 对、1,692 次请求对照**。每格各三对 old→new、三对 new→old，runner/controller/provider/product 统一继承 CPU 2、8 限制；两个逻辑 CPU 属于不同物理核且记录的 CPU 类型证据一致。记录 runner 调用线程的实际 affinity 和前后拓扑；未独立持续采样每个工作线程，也不宣称机器完全独占。

CPU affinity 与[旧的不限核心批次](2026-10-30-inline-content.md)不同，因此本轮只将 fresh Rust 配对用于效果判断。Pi `90d863f7c1183b965f099855715d6e88c7b8a52c`（`packages/durable`）与 DeepSeek `687f282e8625c6b8d11ff58211a90524be2e4317` 的 60/12 个固定参考继续保留，但不把本轮限制核心的数据除以旧参考生成新的追平倍数。

DeepSeek SDK idle 不等待最终 flush/fsync，缺少同等 partial-token 文本发布；其退出 CPU 和完整进程写入不可得边界不变。Rust/Pi 请求核心对照排除工具定义（7 对 4）；AppServer transport 不在该进程内性能基准中。

## 首批测量结果与反例

普通 O3/default-feature 候选二进制 SHA-256 为 `4a141e7199734cf74cdba0320280c936a973e28a581f96c8cc4992858bc287d6`。144 产品、72 对、1,692 请求对照全部通过独立 strict audit；8 个独立诊断产品也通过。以下变化是逐对百分比的中位数，不是两批中位数相除。

| 模式 | 场景 | wall 变化 | 请求窗口 CPU 变化 | 完整进程写入变化 |
|---|---|---:|---:|---:|
| 无实时订阅 | 8 历史轮 | −1.539% | 0.000% | −0.439% |
| 无实时订阅 | 64 历史轮 | −3.601% | −7.964% | −0.437% |
| 实时订阅 | 8 历史轮 | −3.140% | −2.817% | −0.433% |
| 实时订阅 | 64 历史轮 | −14.069% | −11.855% | −0.424% |
| 无实时订阅 | stream | +0.946% | +3.846% | −0.246% |
| 实时订阅 | stream | +1.621% | +14.091% | −0.246% |

十二格 wall 中位数十降两升，逐对为 **50 降、22 升**。stream 两种订阅模式各有 5/6 对延迟回退；live stream 的请求窗口 CPU 为五升一同，完整 CPU 为五升一下、配对中位 **+13.636%**。完整 CPU 的原始批次中位数是 220→240 ms；不能把配对中位数误写成这两个数相除的变化。两种运行顺序下 live stream 完整 CPU 中位数均回退，不能简单归为顺序。

完整写入 **71 对下降、1 对上升**；唯一上升是 live/cancel pair 2（从 0 编号），+110,592 bytes（+4.079%）。stream 每对减少 8,192 bytes。RSS 有 47 对上升，不能宣称内存全面改善。[完整十二格、逐对反例和固定历史参考](2026-10-30-shared-domains-tables.md)保留所有值。

## 为什么继续修复冷路径

stream fixture 有 128 次门控 delta，每片 18 字节，总计 2,304 字节；每片都会改变 block，因此持续触发缓存 miss。源码复核确认三个新增成本：

- Attempt 的 model/settings/live block/Tool 单项调用计算 `fit`，调用者却丢弃它；整个 Attempt 随后仍执行整体预算计算。
- 新 live block 的规范化 DTO 存入缓存后，又由 `into_owned()` 复制到输出；原路径直接移动规范化值。
- `Items::default()` 先分配 model/settings 两个 owner，相等时立即丢弃并换成原 owner。

这些是确实存在的重复工作，足以支持针对性修复；现有样本不能量化每项分别造成了多少回退。首批不删除、不与后续数据混池，也不靠调整 fixture 或持久性来掩盖回退。后续版本采用专用 admission 复用和可验证的 live 证书，另行冻结源码、检查、构建和测量。

## 写入诊断

| 每 32 个热轮次 | short-small：旧→初始候选 | short-large：旧→初始候选 |
|---|---:|---:|
| 主库 write commits | 160→160 | 160→160 |
| WAL frames | 5,737→5,740 | 6,446→6,461 |
| 全主库逻辑列值增长 bytes | 1,103,370→1,071,066 | 1,129,497→1,096,972 |
| Statistics 列增长 bytes | 73,016→57,038 | 74,047→57,483 |
| Execution 列增长 bytes | 46,888→30,548 | 47,048→31,168 |

两个私有列合计缩小约 27%，全库逻辑列值增长约降 2.9%；**这批独立 WAL 帧没有下降**。页布局、索引和每轮 5 次写提交仍支配物理写入。列值字节、端点页归属、WAL 帧、进程 write_bytes 和设备流量不同；诊断单次产品也不等于正式六对测量，不能用逻辑压缩比例替代实际 I/O 收益。

## 可复核证据

[首批归档](evidence/shared-domains-runtime-20261030/README.md)为 1,104 个 payload 文件、10,602,364 个压缩字节，SHA-256：`0d10c07fdcc7666950097d4806590fef9c869822a6181570cf464d5ffe3f7e71`。新目录解压后逐文件哈希、安全成员校验、formal/diagnostic 原始重算及完整统计表比对全部通过；[portable receipt](evidence/shared-domains-runtime-20261030/portable-verification.json)为 accepted。复算不执行产品、编译器或 Pi/DS。

失败编译/lint 记录与所有不利样本均保留。没有运行全仓库 CI/browser E2E，也没有合并 main；PR 原有冲突仍在。
