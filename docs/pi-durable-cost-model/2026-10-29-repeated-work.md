# 重复工作合并首批候选：实现与验证

这是生产提交 `57304c804` 的首批候选报告；后续排序置换修正及其独立测量见 [终版报告](2026-10-30-repeated-work-final.md)。首批将已确认的重复工作落地：合并请求几何遍历、复用 metadata 数组计算、批量读取 Trace header、复用相邻成员捕获、加速紧凑记录的公开长度校验，并删除重复 Attempt 索引。**写入有所减少，整体延迟仍未稳定改善，也未追平 Pi。** 正式 60 对中 59 对完整进程写入下降，12 个场景中位数下降 0.732%–2.166%；延迟六格改善、六格回退。live 长历史延迟中位数下降 12.690%，live 大正文则增加 19.800%，不能将局部收益宣称为整体加速。

生产提交为 `57304c8047fc49894aa502dbdda747c6ad6180f3`，src tree 为 `6510710ef835d3854cb8fe4906b47d3144172619`；普通 O3/default-feature 二进制 SHA-256 为 `453556bd6aae4735d4dfd5f8555eeabb14da17e1d9fe74987088ef46d3e87273`。本轮 fresh baseline 为前轮生产提交 `978ff5554f17b2214872addd6090ff4278143378`，二进制 SHA-256 `b6d283c8e478a79a3dc51740a00eb4c8f593d522d548ce3e4eb89a704f6caf14`。后续文档提交不改变被测源码。

完成的改造及保留的功能边界如下。这里的次数是源码和工作计数回归证明的变化，不是预估的百分比收益。

| 改造 | 减少的重复工作 | 保留的规则与成本 |
|---|---|---|
| 请求 fragment 几何捕获 | 新消息模板的边界查找、prefix 和 suffix 捕获由三遍合为一遍；suffix 变体、首次 stable scalar 捕获由两遍合为一遍 | 独立 fragment 数值/深度准入、F/C 顺序、相对坐标、原始块、hash 和失败回滚不变；prefix 完成后才让 suffix 使用同一 TEMP workspace |
| Metadata 数组复用 | 对相同且有序的不可变 owner 复用排序、聚合预算及精确长度；借用路径集合替代逐项线性查重，合并 frontier 遍历 | 仍作 O(N) 指针比较和 Arc 引用复制；每类只缓存最新数组，不保留历史链；保留重复输入、首来源优先级、cut 和 fit/eviction 后重算 |
| Trace header 批量读取 | 最终已排序/去重/截断的 1–32 条 header 从 N 次 SQL 合为一次，空页不查询 | 各 kind 的候选 seek、洞检查、owner anchor 读取和逐条验证仍在，错误顺序及历史 cut 不变；不是把整页所有 SQL 都降为一次 |
| 相邻成员捕获复用 | 同一次 final settlement 的 canonical → terminal 命中时省一次 members 解码 | 仍查询洞及实际最新 head、读取 blob 并精确比较字节；不同 head、改写、损坏、触发器和回滚继续按实际存储处理，terminal 新值仍编码 |
| 紧凑记录读取校验 | 对三个封闭 DTO，正向预算证明通过后不再执行实际 JSON 数字/标点编码来计量长度 | MessagePack 结构预检、typed decode 和 EOF 检查保留；保守 false 或未知仍回退实际 JSON 长度，保留密集浮点、宽 opaque 整数及精确边界的可读范围；writer/attach 准入不变 |
| 删除重复 Attempt 索引 | 删除 `events_attempt_idx(attempt_id,sequence)`，每条 attempt_id 非空的新事件少维护一个索引条目 | 共享 `events_attempt_kind_idx`；执行历史 11 个、lineage 6 个固定 kind 的有序流由 SQLite 合并，保留原生 owner 筛选、损坏顺序和 cut；存在分支启动成本，无全历史临时排序 |

SQLite 仍为 **WAL/NORMAL**，schema **83 → 84**，不加入旧 schema 兼容或迁移。稳定 scalar 的完整编码身份仍必须覆盖 configuration owner 之外的 invocation/context/profile 等字段，不能用不完整身份跳过校验。原生 kind 与 Trace kind 的索引仍独立：有效原生 type 被改写后，Trace 仍须找到并验证自己独立保留的事实，不能因原生 kind 变化静默漏读。

**正式结果。** 仅新跑 Rust：六场景 × 两种订阅模式 × 五对交错运行，120 个产品、60 对、1,410 次请求比较。下表为每对百分比变化的中位数，wall 方括号为五对最小/最大值；不是两批中位数相除。wall 使用各产品测量轮次的中位延迟，完整 CPU/写入截至 cleanup 采样。

| Mode | Case | Paired median wall % [min,max] | Full CPU % | Full write bytes % | Sample anon PSS % | Sample RSS % |
|---|---|---:|---:|---:|---:|---:|
| no-live-subscriber | short-small | -1.172 [-43.017,16.122] | -0.952 | -2.165 | -0.170 | -0.178 |
| no-live-subscriber | short-large | 6.200 [-16.786,54.280] | -0.787 | -1.677 | -0.017 | -0.162 |
| no-live-subscriber | stream | 0.185 [-3.175,2.369] | -12.000 | -0.732 | -0.027 | -0.219 |
| no-live-subscriber | large | -4.565 [-8.001,3.896] | -6.250 | -0.831 | 0.137 | 0.048 |
| no-live-subscriber | tool | 1.695 [-20.546,4.987] | 0.000 | -1.449 | -0.610 | -0.847 |
| no-live-subscriber | cancel | 1.413 [-0.878,45.368] | 0.000 | -0.857 | 0.062 | -0.456 |
| live-subscriber | short-small | -1.813 [-40.647,37.785] | 1.987 | -2.166 | 0.164 | -0.127 |
| live-subscriber | short-large | -12.690 [-43.427,96.579] | -3.310 | -1.734 | 0.587 | 0.439 |
| live-subscriber | stream | -1.690 [-2.389,1.444] | -6.897 | -0.732 | -0.062 | -0.282 |
| live-subscriber | large | 19.800 [-16.561,49.090] | 12.500 | -0.831 | -0.018 | -0.276 |
| live-subscriber | tool | 7.879 [-17.518,42.550] | 0.000 | -1.449 | 0.831 | 0.343 |
| live-subscriber | cancel | -3.208 [-5.896,12.742] | 0.000 | -0.857 | 0.119 | -0.075 |

完整写入单对范围为 **−2.822% 至 +0.122%**。唯一增加是 no-live stream 的 pair 2：3,358,720 → 3,362,816 字节（增加 4,096 字节）；其测量窗口写入也增加 12,288 字节。不能写成 60/60 对都下降。

延迟 29 对下降、31 对上升，12 格五对范围均跨零。live 大正文的测量窗口 CPU 配对中位数为 0%，完整 CPU 为 +12.5%；五个产品合计完整 CPU 770 → 840 ms，其中窗口内 200 → 210 ms、窗口外 570 → 630 ms，不能把完整 CPU 的增量全部归因到请求内 metadata。live Tool 的测量窗口 CPU 中位数为 +20%，完整 CPU 中位数为 0%，这项不利结果同样保留。CPU 端点计数粒度为 10 ms，未作显著性或因果结论。RSS 九格中位数下降、三格增加，单独的匿名 PSS 不等于 RSS。

| 热请求模式 | 历史轮数 | 测量窗口 CPU 配对变化 | 当前中位延迟 ms | 中位延迟 / 固定 Pi | 完整写入 / 固定 Pi |
|---|---:|---:|---:|---:|---:|
| no-live-subscriber | 8 | +0.000% | 8.767 | 2.796 | 4.056 |
| no-live-subscriber | 64 | +5.263% | 13.744 | 4.166 | 4.551 |
| live-subscriber | 8 | +0.000% | 16.327 | 4.774 | 4.056 |
| live-subscriber | 64 | -9.877% | 17.672 | 4.991 | 4.549 |

固定 Pi 的热请求中位延迟比为 **2.796–4.991 倍**。这是跨批次描述性比值，不能拿前轮的比值变化替代本轮 fresh Rust 配对效果。完整数值、所有逆向样本和不可用指标见 [结果明细](2026-10-29-repeated-work-tables.md)。

**写入减少来自哪里。** 独立 WAL 诊断每格测量 32 轮，两版均为 160 次主库写提交，即每轮 5 次。端点页面归属显示，删除的 Attempt 索引对应的帧消失；其他事件索引仍需维护。

| WAL/存储指标 | 8 历史：旧 → 新 | 64 历史：旧 → 新 |
|---|---:|---:|
| 总 WAL 帧 | 6,217 → 6,065（−2.445%） | 7,104 → 6,958（−2.055%） |
| events 索引合计帧 | 1,858 → 1,711 | 2,139 → 1,982 |
| events_attempt_idx 帧 | 147 → 索引删除 | 157 → 索引删除 |
| request_source_pages 自动主键索引帧 | 84 → 79 | 162 → 173 |
| events 表帧 | 288 → 288 | 288 → 288 |
| content_segments + content_frontiers 帧 | 874 → 874 | 1,173 → 1,173 |
| request_source_pages 逻辑值字节 | 156,393 → 156,393 | 150,944 → 150,944 |
| 全主库逻辑值字节 | 1,121,704 → 1,121,738 | 1,147,634 → 1,147,527 |

逻辑值、索引页面、WAL 帧和进程累计 write_bytes 是不同口径。此轮没有再次压缩公开/原生内容；小幅逻辑字节差异也不能宣称存储压缩。WAL 归属是端点页面分类，不是每次写入瞬间的严格归因；导航页自动主键索引帧在长历史反而增加 11 帧。其余端点归属帧计数相同。列诊断每格两版均新增 256 个事件、32 个 request snapshot，导航页记录数保持 229 / 224。

**CPU 诊断及回退复核。** 三类 profile/control 场景为 live 64 历史、no-live 64 历史和 no-live Tool；文件名里的 `large` 是 `short-large`，并不是大正文 `large` 场景。以下是各自一次独立诊断，不能替代正式五对结果。

| 场景 | Control 平均 ms/轮：旧 → 新 | Profile 平均 ms/轮：旧 → 新 | Profile 总样本：旧 → 新 |
|---|---:|---:|---:|
| live，64 历史 | 17.084 → 11.629 | 19.938 → 11.176 | 220 → 122 |
| no-live，64 历史 | 14.522 → 17.606 | 10.297 → 10.021 | 95 → 91 |
| no-live，Tool | 39.790 → 50.509 | 36.143 → 55.780 | 10 → 15 |

live 长历史中，publication encode 的 inclusive 样本为 38/220 → 19/122，read_with_trace 为 27/220 → 15/122，请求 read-layout 为 16/220 → 7/122。它们会重叠，不能相加成总成本或当成各改造的因果收益。采样频率为 250 Hz，Tool 仅 10/15 个样本；零样本不等于零成本。

对回退进行了源码复核：metadata 新缓存保留的是引用、排序结果和预算，没有增加正文的深克隆；几何合并没有增加正文编码或落盘次数。正常三个封闭 DTO 已通过 writer 的同一预算证明，witness 的 128 字节预留也覆盖附加 transcript cursor，因此读侧 false→精确长度双走不是这些正常新写记录的预期路径。结构预检仍须在 typed 分配前拒绝恶意长度及非 JSON 类型。一次预算遍历不保证其常数开销一定小于精确编码，这些源码事实也不足以解释大正文 +19.800% 的回退；该回退保留为未归因结果，不被称为噪声，也未通过挑选重跑删除。

当前仍执行实际 checkpoint 输出、必要的 owned snapshot replacement/eviction、原生恢复事实、请求/Tool 来源与坐标校验、可独立分页的派生域和索引。这说明为什么合并重复步骤并不等于达到 Pi 的整体成本；本轮也没有证明这些功能性工作已达到理论最小成本。后续复核确认 metadata 排序缓存新增了第二份 Arc 向量及每次输出的引用复制；live 变化页可能持续 miss。这是确切的额外 bookkeeping，虽不能据此归因墙时回退，仍纳入终版修正。首批结果保留，不与终版合并。

**固定参考保持不变。** Pi Durable 为 `90d863f7c1183b965f099855715d6e88c7b8a52c`（`packages/durable`），DeepSeek Harness 为 `687f282e8625c6b8d11ff58211a90524be2e4317`。本轮未重跑两者。相对固定 DeepSeek SDK idle，live short-small / short-large / stream / large 的平均延迟比为 **1.134 / 1.178 / 0.987 / 0.259**。DeepSeek idle 不等待最终 flush/fsync，也不提供相同的 partial-token 文本发布；其退出/flush wait4 CPU 单列，完整进程写入不可得，不当作零。Rust/Pi 核心请求等价检查排除工具定义（7 对 4）；AppServer transport 不在此进程内基准中。

**验证与复算。** 2,270 项测试通过、0 失败、1 项既有忽略；fmt 和全 target/feature Clippy（`-D warnings`）通过。15 步成功队列的 948 个 src/tests/Cargo 输入前后相同。新增 19 项回归覆盖上述各项，198 项关键回归均被审计核对为实际通过。最初一次测试导入错误和随后 Clippy 失败的日志完整保留，修正后的完整队列才用于构建准入。正确性使用 release 依赖/root O0/all features；性能使用另一个干净的普通 O3/default-feature 构建。

严格审计接受 120 个正式产品 / 60 对 / 1,410 次请求比较，以及 20 个独立诊断产品。原始数据、脚本、源码与二进制身份、失败记录、固定参考 pins 和结果保留在 [可复算证据](evidence/repeated-work-runtime-20261029/README.md)。归档含 **1,129 个 payload 文件**，压缩为 **11,245,696 字节**，SHA-256 为 `bcaa813429c8dc564e9d032d42c328ecda2a3d8570c563af7d7ac82fd7ade090`。归档已在全新目录完成逐文件校验，并独立重算正式、诊断和结果表，`portable-verification.json` 回执为 accepted；重算不执行产品或编译器。

未运行全仓库 CI/browser E2E，也未合并 main；PR 原有冲突仍在，继续保持 draft。当前证据支持重复工作及写入的具体减少，支持 live 长历史的本批配对改善，**不支持整体延迟已改善或已经追平 Pi**。
