# 合并重复操作后的性能与功能验证（2026-10-28）

本轮合并了重复的工具注册验证、同事务 cut 读取、transcript 准备、生命周期读取、Turn/Trace 更新，并让不能进入缓存的 response 直接移动给调用者。结果是部分场景改善，仍有明确回退，尚未整体追平 Pi Durable。四组 warm 请求的配对耗时变化依次为 **−36.94%、+7.35%、−16.26%、−13.33%**；无订阅 tool 的五对结果全部变慢。物理写入基本未变，不能把删除 SQL 和复制操作表述为全面加速。

完整数值、范围和所有不利配对见[生成表格](2026-10-28-merged-operations-tables.md)。正式结果与独立诊断分开统计；Pi 和 DeepSeek Harness 按用户要求只引用固定旧基准，没有重跑。

**测量版本与范围。** 新旧 Rust 每格五对交替运行，共 120 个产品、60 对、1,410 次请求比较。正式采集为 2026-10-28 10:13:24–10:16:27 UTC，随后另做 12 个诊断产品。两种 Rust 均为 schema 81、SQLite WAL/NORMAL，普通 release O3/default features；本轮未改变提交边界，没有迁移或兼容解码器。

| 身份 | 旧 Rust | 本轮最终 Rust |
|---|---|---|
| 生产提交 | `459b1b6005bfbf7f0a1a1d52df5a6900b06c56c6` | `0baac4e2e144ea7a9c518798ded4c16123fbb680` |
| src tree | `955c5220e821babe9ece0cdb4791164056a3056f` | `8ea0198d2a03608e83bab958659d269c963ab40a` |
| binary SHA256 | `21d50ee8ac86bb80a2132e6013ece360ae56944d311abbaf87c0576ef32c0e57` | `99f833f043cde941cc27dee2f5d3de93dcd0af03fc070219012c4675fe494916` |

构建使用仓库固定 Rust/Cargo 1.98.1。功能检查使用独立的 release dependencies/root O0/all-features 构建，性能产品不使用这个检查二进制。构建日志、原始请求、产品边界、检查输入与二进制身份均被严格审计绑定。

**具体合并。** 以下工作由 `ccef6e887` 和 `0baac4e2e` 完成；保持原生权威、损坏检测、历史读取、预算与恢复约束。

1. [工具注册表](../../src/tools/executor.rs) clone 和按模型筛选时共享已验证的不可变注册项，只重建 ID/名称索引，不再重新注册并验证 schema。真正的新注册仍完整校验，executor、normalizer、权限与并发策略保持原绑定。
2. [ValidatedCut](../../src/durable/sqlite/observation.rs) 把 transcript、response、statistics 和 Tool 读取的 cut 证明绑定到同一个实际 Transaction，合并重复 J/L/T（Journal/Ledger/Transcript）前沿读取。同连接写入或事务升级使证明失效；发生写入后，即使随后回滚到 savepoint，证明仍失效。公共独立入口仍验证 cut，洞检测、逐 Tool 原生归属及类型检查、错误顺序仍保留。
3. [逐行与整页 transcript 准备](../../src/runtime_client/checkpoint/prepared_transcript.rs) 只复用 descriptor 字节、坐标一致且持有同一已认证 response owner 的完整行。移动页面可继承不变行；total 和有序 row owner 指针均不变时，共享一个有界整页准备结果。规范化正文以 Arc 共享，预算证书由已准备行组合，避免第二次整页 preflight。运行中、Tool 及未证明行重新处理；预算不足走原逐行准入，旧 cut、精确 JSON、metadata 和 revision 语义保留。最终 owned 输出与 wire 编码仍有成本。
4. [response reader](../../src/durable/sqlite/observation_response.rs) 返回 owned value 与可选共享 owner。只有实际准入缓存才分配共享 owner 并保留必要副本；running、provisional、delegated、历史或超界等未准入结果直接移动，不再无条件 Arc::new 后深拷贝。缓存仍限 32 项，不放宽认证或失效规则。
5. [生命周期写入](../../src/durable/sqlite.rs) 在首次生命周期写入前复用第一阶段的存在性结果。首次 INSERT/UPDATE 后，后续 key 重新查询实际存储，保留触发器的可见影响；全部终态检查仍在首次写入前完成。
6. [Turn 安装](../../src/durable/sqlite/observation_turn.rs) 在原可选 savepoint 内将 ordinal/header/anchor 合并成一次 UPDATE，空初始 prompt 只读一次。后续 adoption 仍读真实来源；任一成员失败整组回滚，原生事件与独立派生布局继续保留。
7. [Trace 安装](../../src/durable/sqlite/observation_trace/event.rs) 的实际 descriptor 未变时跳过重复 summary/proposal UPDATE；[canonical seal](../../src/durable/sqlite/observation_trace/canonical.rs) 复用第一次成功编码的字节。仍读取、解码和核对真实存储，仍检查 typed 规范化后的 16 KiB 准入。原字节未超限不能代替重新编码后的预算检查，因此这不是“零序列化成本”。

逐行复用最初丢失了不变整页的一次复用；未准入 response 也仍多做 Arc 构造和复制。终版已经合并这两条重复路径，保留移动页面的逐行复用。这是源码可验证的工作削减，不能据此单独解释所有端到端变化。

**最终正式结果。** 表中百分比都是“五对变化百分比的中位数”，不是两个产品中位数之比。Wall 指每个产品的 measured-turn median；完整 CPU/写入截至记录的 cleanup 采样点。每格只有五对，范围较宽，不据此给出稳定收益或显著性保证。

| 观察模式 | 场景 | Wall 配对变化 [最小, 最大] | 完整 CPU | 完整写入 | 匿名 PSS | RSS |
|---|---|---:|---:|---:|---:|---:|
| 无 live 订阅 | 8 轮历史，短请求 | −36.944% [−45.108%, +24.770%] | −15.385% | −0.019% | −0.970% | +0.152% |
| 无 live 订阅 | 64 轮历史，短请求 | +7.351% [−37.238%, +60.784%] | −0.488% | +0.018% | −0.284% | +0.402% |
| 无 live 订阅 | stream | −0.552% [−1.645%, +0.374%] | −3.704% | 0% | −0.370% | +0.565% |
| 无 live 订阅 | large payload | +0.054% [−2.040%, +3.743%] | −6.250% | 0% | −0.142% | +1.281% |
| 无 live 订阅 | tool | **+8.527% [+4.421%, +23.003%]** | +6.250% | 0% | −0.360% | +0.764% |
| 无 live 订阅 | cancel | +10.215% [−1.284%, +28.728%] | 0% | 0% | +0.736% | +1.275% |
| 有 live 订阅 | 8 轮历史，短请求 | −16.261% [−49.232%, +13.739%] | −13.587% | +0.019% | −0.034% | +0.946% |
| 有 live 订阅 | 64 轮历史，短请求 | −13.333% [−48.908%, +56.384%] | −15.091% | +0.025% | +0.300% | +0.814% |
| 有 live 订阅 | stream | −0.961% [−2.451%, +1.723%] | 0% | 0% | +0.370% | +1.374% |
| 有 live 订阅 | large payload | −12.252% [−27.250%, +2.436%] | 0% | 0% | +0.248% | +1.028% |
| 有 live 订阅 | tool | −3.785% [−10.159%, +46.354%] | +5.000% | 0% | +0.472% | +1.315% |
| 有 live 订阅 | cancel | −4.774% [−27.722%, +32.586%] | 0% | 0% | −0.281% | +0.905% |

Warm 的绝对值更能说明仍有多少开销。下表每个值为五个产品各自统计量的中位数，每产品 32 次测量请求；CPU 是测量窗口除以 32，不是完整 CPU。

| 观察模式 / 历史 | 旧→新 median wall，ms/turn | 旧→新测量 CPU，ms/turn | 测量 CPU 配对变化 | 新 median wall / 固定 Pi |
|---|---:|---:|---:|---:|
| 无 live / 8 | 17.4805 → 10.5430 | 19.0625 → 14.0625 | −26.230% | 3.363× |
| 无 live / 64 | 17.0325 → 17.7015 | 17.8125 → 19.6875 | **+12.727%** | 5.366× |
| 有 live / 8 | 20.0175 → 17.7405 | 30.0000 → 24.6875 | −22.549% | 5.187× |
| 有 live / 64 | 22.3765 → 19.3930 | 32.5000 → 26.8750 | −17.308% | 5.477× |

无 live/64 的 wall 和测量 CPU 均有 4/5 对变差；完整 CPU 含 64 轮历史预热，不能用 −0.488% 掩盖测量窗口的 +12.727%，也不能据此断言只是等待增加。无 live/tool 的 wall 和生命周期耗时都 5/5 变差，生命周期配对变化 +5.384%；有 live/tool 的 wall 虽下降，完整 CPU +5%、生命周期 +6.146%。无 live/cancel wall +10.215%；live/stream 生命周期 +1.354%；live/cancel 测量 CPU +25%（中位数 40→50 ms，受到 10 ms tick 粒度影响）。全部 12 格 RSS 配对中位数上升 0.152–1.374%，没有整体内存下降证据。这些不利结果与单对反转全部保留，不归因为未经验证的“噪声”。

**独立诊断。** 12 个产品分别是 4 个逻辑列采集、4 个 CPU/control、4 个 WAL 采集；不能混入上面的正式配对。WAL 两种历史各测 32 轮、无 live 订阅，两版本均为 **160 次 write commit，即每轮 5 次**。

| 历史 | WAL frames 旧→新 | frame bytes 旧→新 | frame 变化 | 进程测量窗 write_bytes 旧→新 |
|---|---:|---:|---:|---:|
| 8 | 7,050 → 7,047 | 29,046,000 → 29,033,640 | −0.04255% | 34,689,024 → 34,676,736 |
| 64 | 7,918 → 7,924 | 32,622,160 → 32,646,880 | +0.07578% | 39,694,336 → 39,784,448 |

同一事务内减少 UPDATE，不一定减少提交时写出的脏页。当前事件表及其索引的端点归类分别为 2,975/7,047 和 3,232/7,924 frames（42.22%/40.79%），对应计数在两版本未变；这只是 dbstat 端点映射，不是逐时刻精确页归属或 CPU 因果占比。逻辑主库列增量分别为 1,255,587→1,255,596 bytes、1,277,703→1,277,896 bytes；derived source overlay 均仍是 19,968 bytes。本轮没有显著降低逻辑或物理写量。逻辑统计排除 TEMP，并不表示 TEMP 为零；WAL bytes 也不等于进程 write_bytes 或设备 I/O。

live/64 的 250 Hz CPU 采样，测量窗总样本 **177→194**，不能作为 CPU 改善证据。以下为 inclusive 栈计数，互相重叠，不能相加当独占时间：

| 栈桶 | 旧 / 177 | 新 / 194 |
|---|---:|---:|
| PublicationDraft::encode | 44 | 45 |
| ReadDomainCut::read_with_trace | 20 | 28 |
| index_request_read_layouts | 12 | 10 |
| sqlite3LockAndPrepare | 2 | 5 |
| Capture::prepared | 12 | 11 |
| transcript row capture | 0 | 9 |
| registry_for_model / registration / schema validation | 2 / 1 / 1 | 0 / 0 / 0 |

新 transcript head 桶也为 9，但匹配含 row capture，不是另外 9 个独立样本。certificate replay 为零样本不能证明无工作，内联和采样均会影响可见符号。未注入 profiler 的控制运行测量 CPU 为 680→860 ms，注入后的运行为 720→800 ms；控制运行平均 wall 为 14.963→17.933 ms，注入后 15.633→16.203 ms。单次诊断与正式五对出现不同方向，均保留；不拿一个 profile 的占比替代端到端因果验证。

**固定外部参考。** Pi 为 `90d863f7c1183b965f099855715d6e88c7b8a52c`，DeepSeek 为 `687f282e8625c6b8d11ff58211a90524be2e4317`。固定 60 个 Pi、12 个 DeepSeek 产品来自此前已经接受的批次；以下都是跨批中位数之比，不是本轮跨引擎配对估计。

四组 warm 的 median wall 仍为 Pi 的 **3.363/5.366/5.187/5.477×**；完整 CPU 为 **1.170/2.759/1.639/2.813×**，完整写入为 **4.744/5.228/4.744/5.230×**，顺序同 warm 表。其余八格 median wall/Pi 为 0.341–0.917×，“约五倍”不能泛化到所有工作负载。

| live 场景 | 当前 mean ms/turn | 固定 DeepSeek mean ms/turn | wall 比 | 测量 CPU 比 | 测量写入比 |
|---|---:|---:|---:|---:|---:|
| 8 历史短请求 | 16.825 | 15.541 | 1.083× | 1.362× | 50.458× |
| 64 历史短请求 | 18.558 | 15.450 | 1.201× | 1.536× | 58.144× |
| stream | 1,591.861 | 1,583.218 | 1.005× | 0.500× | 50.714× |
| large payload | 35.091 | 165.613 | 0.212× | 0.200× | 35.143× |

DeepSeek SDK idle 不等待最终 flush/fsync，缺少部分 token 文本发布；工具和生命周期边界也不同。exit/flush CPU 单列，缺失的完整进程 write_bytes 不填零。Rust/Pi 采用相同 fake HTTP/SSE 脚本：短请求 Text+Finish 不加延迟，stream 有 128 个 10 ms gate。Rust 结束点包括 settlement signal 与 live terminal，Pi 等待已提交 settlement；Rust 无 live 仍有 attachment/checkpoint/projection，Pi 对应模式不 watchEvents。Rust 默认 7 个工具、Pi 4 个；请求审计比较剔除 tools 字段后的核心请求语义，不等于工具能力完全相同。不宣称三者功能、持久性、响应完成边界完全等价。

**仍未消除的主要工作。** checkpoint 仍需读取 descriptor、解码、检查 cut/provenance、缓存版本与失效条件、execution/member 洞，并读取 statistics、occupancy、trace 与 projection。完整证据允许复用 owner，但最终 owned 输出、metadata 复制/revision 标记及 wire 编码/预算仍执行。终版 PublicationDraft::encode 的 inclusive 样本是 45/194，说明该路径仍值得优化，不能把 23.20% 当可直接节省的独占 CPU。

原生事件 INSERT 后仍有分属不同失败域的 aggregate、execution、source/witness、Trace、Turn 派生安装，身份、时间、计数字段仍有重复，索引也仍多次维护。本轮没有完成“所有字段预先准备再一次 INSERT”的重构，原生/派生 kind 索引未合并，也没有加掩盖缺失证据的 assembling 标记。此前提到的 544 是某些瞬态洞/缺失索引的 WAL frames，不是 544 次检查，不应误写成本轮统一“检查次数”。当前数据不支持“安全优化空间已耗尽”，也不支持“与 Pi 的差距全是功能必需”。

另一处明确的覆盖缺口是 Tool/result 及带 tool_call 的 assistant 行没有当前 TranscriptRowProof，每次 checkpoint 刷新仍创建新 row owner；若安装新 cut 或其他域改变，后续 capture 会重新准备这些行。不变 owner 的整页复用并未覆盖它们；相对旧版基于 encoding_equal 的整页比较，新 owner 策略删除全页深比较的同时，也收窄了旧版原可继承的这类行的范围。旧版同样只继承 encoding_equal 通过的页，浮点或非空 JSON map 本来就不能继承，因此不能认为所有旧 Tool 页都会命中。新行还有 row→page→publication 的 metadata 重放及 Arc 构造成本。两个观察模式都 attach 并取得 checkpoint，无 live 模式也可能进入这些路径。上述是源码可见的成本机制；当前没有无 live/tool 或无 live/64 的独立调用计数及 CPU 栈，不能把它们量化成 +8.527%/+12.727% 的因果解释。两项明确回退尚未因果定位，不能仅用 live/64 profile 替它们解释原因。

**功能检查与失败记录。** 最终 `checks-retry1` 共 14 步 exit 0：**2,148 passed、0 failed、1 ignored**，fmt 和 all-target/all-feature Clippy `-D warnings` 通过。输入前后 928 份 src/tests/Cargo 文件哈希一致。测试包括 SQLite 532、runtime client 564（另 1 ignored）、conversation runtime 96、read-layout 26、content 27、agent 585、ToolRegistry 19、recovery 14、真实进程死亡 72、durable 集成 151、process 集成 62。本轮增加 21 项针对性回归，覆盖工具共享、事务证明失效、逐行及整页准入、精确 JSON、触发器、失败隔离与临时 response 移交。

最终第一次检查真实发现并修复了生产错误：新空页测试期望 required=0，实际为 98304。共享证书重放错误地把已消耗预算加给零预算证书。修复使零 required 保留已有 required、同时重放 metadata；测试预期没有为通过而放宽，完整重跑才得到最终通过结果。初次失败原样保留。

第一候选 `ccef6e8871dda72626e85cb257ec39b32490f300` 有独立 120 个正式、12 个诊断产品和真实 portable 回放，见[第一候选证据](evidence/merged-operations-runtime-20261027)。其四组 warm 配对 wall 为 −10.615/−2.595/−5.377/−7.585%，但 live/8 完整 CPU +8.589%，live/large payload wall +8.848%，live/tool wall +6.081%，stream 完整 CPU +9.091/+10.0%。这批不被终版覆盖或混算；源码复核后补整页复用与临时 response 移动，不声称这些修改已解释或消除所有回退。第一候选更早的编译、fixture、Clippy 失败及构建身份查询目录修正也保留：实际 pinned 1.98.1 被错误记录为查询缓存目录所得系统 1.96，交叉核对实际二进制后修正记录，binary SHA 未变，不称为重建。

**可复核证据。** [最终证据目录](evidence/merged-operations-final-runtime-20261028) 包含原始正式/诊断结果、失败记录、构建身份、冻结脚本及 portable verifier。最终 strict formal/diagnostic 审计均 accepted、errors=[]。独立复算 960 项核心原始计数计算和 228 个配对中位数与生成报告一致。

最终 archive 为 **11,181,372 bytes**，包含 **964 个 payload 文件 / 1,205,001,223 bytes**，SHA256 `e0fa21c9c5115c19a2010ebe15a820e08ac5fe914ce28a32a5e77e16778da0b2`。实际 portable 复验 **accepted=true、errors=[]**，见目录中的 `portable-verification.json`：已重新解包、校验每个文件，并从原始记录重算正式/诊断/报表，全部与严格结果相符；回放后原始文件哈希不变。回放不运行产品、编译器或历史引擎。

第一候选 archive 为 11,085,192 bytes、1,000 个 payload 文件，SHA256 `8514c67739b1236e37a4229cc7308cac2d331a4833143681b28329be1c4f87da`。终版中的 first-candidate 文件只做完整性保留，未再执行第一候选审计；第一候选目录有自己的实际 portable 回放记录。固定 Pi/DeepSeek 的旧原始归档仍独立保存在 [pi-flow-runtime-20261027](evidence/pi-flow-runtime-20261027)，24,024,636 bytes，SHA256 `eef4c601144965e23fc32b303be67bbccdc7b0197b332d62deba9933840b49ec`，固定报告 SHA256 `902e748b08ff085dc4d1edcd7a38eea181bde0b5542a8972bbd3e459472eb98f`。新归档校验 pins 和旧回放收据，不声称重新采集或复验历史引擎。

CPU `/proc` 累计计数按 100 ticks/s 换算、端点粒度为 10 ms，与 profiler 的 250 Hz 采样频率不同；完整 CPU/写入窗口包括初始化、历史准备、验证和记录点之前的清理，不是单 turn 或退出后 wait4。采样 RSS/PSS 不是精确瞬时峰值。完整仓库 CI、浏览器 E2E 和 main 合并未执行；PR 保持 draft，已有 main 冲突仍存在。
