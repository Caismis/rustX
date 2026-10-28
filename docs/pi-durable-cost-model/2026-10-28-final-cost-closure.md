# checkpoint、Tool 与事件派生成本收尾（2026-10-28）

最终结果：**仍未整体追平 Pi Durable**。本轮完整进程写入在 **60/60 对均下降**，各格配对中位数下降 **4.3%–10.4%**，四组热请求中三组 wall 配对中位数下降、一组略升；无 live Tool、live 大正文等仍有回退。热请求 median wall 相对固定 Pi 为 **2.82–5.10 倍**。以下同时保留正负结果，不把删除重复操作表述为全线加速。

本轮针对三类已识别成本完成实现与验证：checkpoint 编码反复遍历 schema、Tool 行重复规范化、事件派生字段分阶段更新及重叠 head 索引。收尾审查又找到规范消息显示 header 的重复捕获；第一批测量出现延迟回退后继续检查，删除了临时表使缓存失效、纯读取 savepoint、Tool 装饰重复准备和解码时无用编码计划等工作，全部纳入本轮。Pi 与 DeepSeek Harness 只使用固定旧基准，没有重新执行。

**参考源码与取舍。** 分析对象是 Pi 的 `packages/durable`，不是 coding-agent 的 JSONL session manager。[Pi storage commit](https://github.com/earendil-works/pi/blob/90d863f7c1183b965f099855715d6e88c7b8a52c/packages/durable/src/storage/sqlite/storage.ts#L187) 先准备文档操作，再在事务中校验和写入；[session transaction](https://github.com/earendil-works/pi/blob/90d863f7c1183b965f099855715d6e88c7b8a52c/packages/durable/src/session/transaction.ts#L672) 在成功提交后采用准备好的状态，通过指针交换发布。[SQLite adapter](https://github.com/earendil-works/pi/blob/90d863f7c1183b965f099855715d6e88c7b8a52c/packages/durable/src/storage/sqlite/node.ts#L194) 与 rustX 同为 WAL/NORMAL，因此本轮不把差距归结为 rustX 额外 fsync，也没有继续削弱同步级别。

DeepSeek 的 [JSONL handle](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/session/session-persistence-jsonl/src/storage.ts#L183) 在入队前验证、复制事件，live 队列最多有 200 ms 有意等待；[实际 append](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/session/session-persistence-jsonl/src/index.ts#L1407) 写入后执行 sync。固定 SDK idle 基准不等待最后 flush，且缺少部分 token 文本发布。借鉴其准备后批量写入的方式，不把不同的完成边界当成等价持久化性能。

**已完成的改造。**

| 成本 | 本轮处理 | 保留的功能约束 |
|---|---|---|
| checkpoint 编码逐层查 schema | 为有限 DTO 根类型编译并复用不可变编码计划；递归编码直接携带节点，保留字段次序与预算计算 | 精确 JSON、数值规则、null/reference 分支、原字节预算 |
| 只解码也编译整棵编码计划 | schema owner 延迟初始化编码计划，首次真正编码才构造，同一 owner 并发只编译一次 | schema 生成和解码语义不变，编码计划仍不可变复用 |
| 临时 fact spool 使缓存失效 | 固定 TEMP 表在连接安装、记录 schema cookie 前建立；spill 只操作数据 | 只复用安装时 cookie；真实 DDL 仍使 response 证明失效，并使 event guard 重新检查 trigger |
| 纯读取准备开启 savepoint | aggregate、pending Turn、assistant ledger 的只读准备去掉 SAVEPOINT/RELEASE | 实际写入、外层 INSERT 与独立失败域 savepoint 保留 |
| Tool 装饰无效准备与重复预算 | 正文或组合额度不能 inline 时提前跳过装饰编码；同一值已准入后不重复 preflight | 精确长度、safe integer、原规范化和 fallback 输出一致 |
| AppServer 中间 JSON 树 | 直接序列化 typed envelope，删除 `to_value → convert` 整树 | 原生大小限制先于 wire 数值错误；最终传输预算独立校验 |
| Tool/result 与运行中行重复准备 | 复用已为原生准入产生的精确 header 字节作为证明；不可变正文与当前 process/response 装饰分开准备 | 每次实际 Tool 类型、参数、归属、结果与损坏检查；当前 cut 和 metadata 不复用旧值 |
| 事件先 INSERT 再多次 UPDATE | 先准备 aggregate、execution、occupancy、source/witness、Trace、Turn、当前 J/T header，再以完整行一次 INSERT | 同一事务；真实失败回滚后仍按原生优先、独立派生失败域处理 |
| 派生子表重复准备 | 请求 Trace core 只构造一次；依赖当前 J 的 FK 子行在 INSERT 后、同一 savepoint 内安装 | 即时 FK、错误优先级、原生事件仍可存续、独立模型/工具目录 |
| 四棵派生 head 索引 | 用两个 VIRTUAL presence mask 和两棵共享 B-tree 代替四棵 | 历史 cut、attempt 范围、缺失域检测；读取为两次有界 seek，而非历史扫描 |
| 规范消息 header 二次捕获 | 初次成功捕获后保留当前事务内的裁剪前草稿；最终只绑定真实 T/raw，非 Tool block 不重建，相同字节不重写 | 原 content/header savepoint、大小边界、raw 缩短后可恢复正文、首次失败后的最终修复 |

保存点计数回归中，AttemptStarted / AssistantMessageCommitted / AttemptCompleted 的可选域包装调用从 6 / 11 / 6 变为 4 / 8 / 4，三事件合计少 7 对 SAVEPOINT/RELEASE（14 条 SQL）。这是指定事件路径的工作计数，不是所有 turn 的统一事务数量或 WAL 降幅。

普通 delta 在本轮之前已经绕过 `CapturedCheckpoint.into_owned`，因此不能把“删除每次完整 snapshot 深拷贝”算成本轮收益。AppServer direct-wire 路径不在本轮对齐的进程内 benchmark 中，不能用该改动解释其端到端变化。

transcript 行证明最多 32 项，累计保留字节上限 786,432 bytes，包含原 header 与实际准入的 Tool 前缀。完整行仅在同一已认证 response owner 下复用，准入包括完成 owner 和已认证无 response 的 native User；running/inherited 行仅复用正文，终态专属条目仍读取当前状态。超预算或未通过证明时继续正常准备，不跳过原生校验。

规范消息草稿只活在当前写事务中，上限 128 KiB，不是长期缓存。常见 16 KiB 内的头直接复用首次编码字节并移动所有权；只有需要裁剪时才保留有界裁剪前副本。最终仍使用原 16 KiB 裁剪算法。自定义 SQL trigger、首次可选域失败、草稿超限均不使用这个证明。已有草稿的最终写入失败只保留一次可选尝试，不新增自动重试。

事件的健康路径以一次 INSERT 写齐该事件行，不再逐域 UPDATE 同一行。自定义 main/TEMP trigger 保留原来可观察的原生优先顺序；guard 只复用最初安装的 schema cookie，不缓存后来可能在 rollback 后复用的版本。真实派生 CHECK/FK 失败仍保留原来的独立失败语义。这里的失败处理服务现有功能，不是旧 schema 兼容层。schema 为 82，不增加迁移或旧格式解码。

**为什么仍保留这些工作。** 本轮发现的可安全删除的上述重复工作已经处理，以下是当前保留项及没有直接删除的原因；其中包括当前实现选择带来的成本，不全是理论必要开销：

- 当前实现发布前仍要冻结整个有界 Snapshot DTO：部分域共享，部分 Vec 深拷贝；包括 `capture_working_seed` 的 snapshot 复制、尚无独立 owner 的域比较，以及按当前 cut 重绑定和排序元数据。热读取的 message/transcript head 限制为 8 项，不是复制 64/96 轮完整历史，原始宽内容仍在 durable source/ContentRef；这些保证离锁编码与历史 seed 的独立性，不能直接删掉或借用继续变化的状态。它们与普通 delta 已避免的最终 checkpoint materialization 是两件事。
- 首次真正使用一种编码根类型仍会建立编码计划，以换取后续编码不重复遍历 schema；本轮删除了解码和并发落败者从未使用的计划构造，保留实际被复用的计划。
- 明确请求的 owned checkpoint 必须产生稳定输出对象；发送结果仍需编码实际字节。编译 schema 和复用正文减少重复准备，不消除输出本身。原生与 wire 数值的长度可能不同，不能仅用一个预算替代两个不同的边界。
- 原始 Ledger/Journal/Transcript、Tool 原始参数，以及各派生域可独立失败的事实需要保留。删除原始记录或让派生域成为恢复权威会改变恢复和损坏隔离能力。
- 当前 cut/provenance、逐 Tool 原生关系和缺失域检查不能用未经重新认证的缓存替代；terminal/process 装饰本来就是可变数据。
- 原生 kind 与 Trace kind 索引不能直接合并。将合法 JSON 中的原生 type 从 A 改为 B 后，Trace 的独立 A 事实仍应可枚举并接受自己的 header 校验；改用原生索引会在校验前静默漏掉它。本轮新增回归固定了此反例。owner 索引也服务不同的有界查询键。再增加差异索引或扫描历史并不能简单免除这项工作。
- 当前 J 的即时外键子行必须在父行之后安装；真实失败域必须能分别回滚。它们保留的多表写入不等于健康路径仍在重复回写同一 events 行。

这些约束解释了具体保留项，不证明与 Pi 的所有性能差距都由功能决定，也不证明任何未来算法都无法改进。下文按实际结果判断本轮收益，不把删除 SQL 次数等同于物理写入或 CPU 等比例下降。

**首批结果与本轮内修正。** 首批生产版本 `584d581e730d3c03c93e71c4010bdeffa387f888` 已完整执行 120 个正式 Rust 产品与 20 个独立诊断产品。完整进程写入的配对中位数下降 4.3%–10.4%，但长历史热请求的 wall 分别回退约 10.0%/14.7%，live Tool 回退 18.4%，live cancel 五对均回退、中位数 8.0%。这些结果没有丢弃，也没有挑选其中较快样本代表最终结果；[原始归档及独立复算](evidence/final-cost-closure-candidate-runtime-20261028/README.md)完整保留。

继续检查发现：晚建 TEMP spool 在旧基线已存在，会使最初安装的 schema cookie 失效，既有 response 缓存因此不能命中，而本轮新增的 event guard 又叠加了反复 schema 检查；Tool inline 路径在正文已失败时仍准备装饰；只读 prepare 仍打开 savepoint；解码也构造从未使用的编码计划。修正只针对这些有代码与工作计数依据的额外操作，不声称它们已解释首批所有 wall 波动。首批 Tool CPU 样本仅旧 15/新 11 个，无法据此对整项延迟作可靠归因。

**最终正式结果。** 120 个新鲜 Rust 产品形成 60 对、核对 1,410 次请求对应关系。百分比为五对各自变化百分比的中位数；wall 是每个产品 measured-turn median，完整 CPU/写入截至 cleanup。五对及其宽范围仅支持描述统计，不能保证稳定收益或完整追平。

| 模式 | 场景 | Wall 配对% [最小,最大] | 测量窗口 CPU% | 完整 CPU% | 完整写入% | RSS% |
|---|---|---:|---:|---:|---:|---:|
| 无 live | short-small | -4.198 [-51.622,+68.623] | +1.786 | -0.769 | -10.398 | +1.079 |
| 无 live | short-large | +0.993 [-48.274,+34.775] | +3.333 | +0.000 | -9.495 | +0.930 |
| 无 live | stream | +0.179 [-2.704,+1.481] | -6.667 | +0.000 | -4.308 | +0.588 |
| 无 live | large | -2.712 [-10.568,+7.844] | +0.000 | -5.882 | -4.481 | +1.047 |
| 无 live | tool | +7.564 [-19.300,+19.661] | +50.000 | +5.882 | -6.993 | +0.969 |
| 无 live | cancel | -1.411 [-18.295,+24.641] | -25.000 | +0.000 | -4.485 | +0.769 |
| live | short-small | -5.446 [-18.243,-3.643] | -11.905 | -7.692 | -10.382 | +0.783 |
| live | short-large | -51.607 [-53.406,+59.767] | -48.113 | -10.940 | -9.532 | +0.304 |
| live | stream | -0.595 [-1.919,+1.396] | +0.000 | -6.897 | -4.308 | +0.834 |
| live | large | +3.428 [-12.517,+68.124] | -25.000 | +6.250 | -4.481 | +0.728 |
| live | tool | -2.625 [-28.555,+11.085] | +0.000 | +0.000 | -6.993 | +0.813 |
| live | cancel | -7.234 [-9.945,+10.730] | -25.000 | -5.882 | -4.485 | +0.988 |

live/64 的 wall 中位变化为 −51.6%，但五对范围为 −53.4% 到 +59.8%，完整 CPU 仅 −10.9%；不能将其解释为稳定减半或某一补丁的独立收益。无 live/8、64 的测量 CPU 分别 +1.8%、+3.3%，与其完整 CPU 口径不同；平均 wall 的配对变化也分别是 +0.8%、+5.5%，不能混用 median 口径掩盖。无 live Tool wall +7.6%、完整 CPU +5.9%；其单 turn 测量 CPU +50% 受 10 ms 累计计数粒度影响，但不因此把回退删去。live 大正文 wall +3.4%、完整 CPU +6.25%。全部 12 格 RSS 配对中位数仍上涨约 0.30%–1.08%。

热请求绝对值（各产品 32 个测量 turn 的统计量，再取五产品中位数；范围见完整表）：

| 模式/历史 | 当前 median ms/turn | 当前 mean ms/turn | 测量 CPU ms/turn | median wall/固定 Pi | 完整 CPU/固定 Pi | 完整写入/固定 Pi |
|---|---:|---:|---:|---:|---:|---:|
| 无 live/8 | 15.386 | 15.724 | 17.812 | 4.908× | 1.372× | 4.251× |
| 无 live/64 | 16.468 | 15.937 | 18.438 | 4.992× | 2.865× | 4.733× |
| live/8 | 17.426 | 18.268 | 24.688 | 5.095× | 1.588× | 4.251× |
| live/64 | 9.979 | 12.382 | 17.188 | 2.819× | 2.933× | 4.730× |

固定 Pi 的对应 median 为 3.135/3.299/3.420/3.541 ms。其余八格 wall/Pi 为 0.325–0.916×，因此“约五倍”只适用于部分热请求，不能泛化为所有工作负载。固定 DeepSeek 的 live mean wall 比值依 short-small/short-large/stream/large 为 **1.175/0.801/0.996/0.195×**；测量写入比值为 **44.917/53.880/47.143/31.857×**。这些保留其原 SDK idle 边界，不构造等价持久化或完整生命周期结论。

[完整数值、每对回退与范围](2026-10-28-final-cost-closure-tables.md)由已通过严格审计的原始结果生成；首批与最终批次不混池。

**独立诊断。** 20 个产品单独采集：逻辑列 4、CPU/control 12、WAL 4，不混入正式配对。两种历史各测 32 轮，主库仍为 **160 次 write commit，即每轮 5 次**；本轮减少触页，没有改变此提交数。此前固定 Pi 的同场景也是 160 次提交，因此不能把当前差距解释为提交次数仍比 Pi 多。

| 历史 | WAL frames 旧→新 | frame bytes 旧→新 | frame 变化 |
|---|---:|---:|---:|
| 8 | 7,054 → 6,322 | 29,062,480 → 26,046,640 | -10.377% |
| 64 | 7,921 → 7,227 | 32,634,520 → 29,775,240 | -8.762% |

主库逻辑列净增长几乎不变：8 历史为 1,255,492→1,255,452 bytes，64 历史为 1,277,995→1,278,018 bytes；native snapshot/event JSON 等主要列增长不变。WAL 减少与合并更新、减少索引维护的方向一致，不能仅凭端点映射将全部帧降幅精确分配给某个域。TEMP 数据不在主库列统计内，VIRTUAL mask 的缺席也不等于测得零成本。四份 WAL 未出现同一事务内的重复页；约 91% 的帧是不同事务再次触及先前页面，不能把它说成同事务反复写同一页或归因于 cache spill。剩余跨事务触页、原生及独立派生索引维护，不能靠继续合并同一行的 UPDATE 全部消除。

| CPU 场景 | 样本总数旧→新 | publication encode | read-with-trace | request read-layouts | Snapshot clone |
|---|---:|---:|---:|---:|---:|
| live/64 | 147 → 217 | 30 → 36 | 26 → 31 | 9 → 18 | 1 → 3 |
| 无 live/64 | 163 → 134 | 31 → 13 | 0 → 0 | 15 → 14 | 1 → 1 |
| 无 live Tool | 15 → 12 | 2 → 3 | 0 → 0 | 3 → 3 | 0 → 0 |

以上是包含对应函数的采样数，桶之间重叠，不能相加。无 profiler 的控制运行测量 CPU（旧→新）分别为 live/64 **940→780 ms**、无 live/64 **670→550 ms**、Tool **60→60 ms**；注入 profiler 后分别为 **600→860 ms、650→530 ms、60→50 ms**。live 的 profile 与 control 方向相反，不能从一个栈比例推断总体收益。Tool 只有 15/12 个样本，无法可靠解释正式 no-live Tool 回退。

新版本三组 guard schema 扫描均未采到，工作计数回归另外证明了真实 spill 后不再触发扫描；零采样本身不证明零成本。Tool 首次编码计划仍采到 2/12 个样本，它是换取后续复用的一次性准备，没有被本轮的“解码零编译”消除。当前 /proc 采样本身在三个 profile 中占 10/18/1 个样本，计入累计 CPU，不直接计入 turn settled wall。

**未达到的结果。** 本轮没有实现整体 Pi 性能追平，也没有消除所有场景回退。无 live Tool、live 大正文的正式 wall/完整 CPU 回退仍在；当前五对及低样本诊断不足以可靠确认其全部成因，不能把它们归咎于已经证明的单一热点，也不能承诺再删哪项检查就会恢复。上文列明的是当前保留项及具体约束，不是“全部差距都不可优化”的证明。

**功能验证。** 收尾复验目录中的最终 `checks-retry1` 的 15 步全部通过：**2,239 passed、0 failed、1 ignored**，包括 SQLite 569、runtime client 575（另 1 ignored）、AppServer 43、conversation runtime 96、read-layout 26、content 27、agent 585、ToolRegistry 19、recovery 14、真实进程死亡 72、durable 集成 151、process 集成 62，以及格式和 all-target/all-feature Clippy `-D warnings`。检查前后 940 份 src/tests/Cargo 输入哈希一致。本轮净新增 51 项针对性回归，覆盖编码精确等价、Tool 复用与新鲜读取、完整事件 INSERT、即时 FK、独立失败、触发器、共享索引历史 cut、显示头预算与损坏后的独立 Trace 枚举；还覆盖固定 TEMP spill 后 owner/guard 仍可复用、纯读 prepare 无额外保存点、Tool 装饰工作计数与精确组合预算、解码不编译及首次并发编码只构造一个计划。

检查使用 release dependencies/root O0/all features；性能使用另外干净构建的普通 release O3/default features。两者身份分别保留，不混用测试二进制做性能结论。早期编译、即时 FK 顺序、测试夹具和 Clippy 失败均保留。末轮新增保存点计数夹具曾错误地经通用 `append_event` 提交专用事务事件，触发原有拒绝；已改用同一真实事务写入函数来验证局部准备逻辑，保留全部隔离与输出比较。两个显示头边界夹具最初把 serde 字节长度误当保守 typed 准入计费，后来按 `required_budget` 校准；最终仍严格比较原完整捕获的字节，未放宽功能断言。T 测试区分“JSON patch 能存下”与“最终 typed 准入仍须裁剪”，不宣称制造了物理 16 KiB CHECK 失败。

**测量口径。** 旧 Rust 为 `0baac4e2e144ea7a9c518798ded4c16123fbb680`，src tree `8ea0198d2a03608e83bab958659d269c963ab40a`，binary SHA256 `99f833f043cde941cc27dee2f5d3de93dcd0af03fc070219012c4675fe494916`。本轮最终生产提交为 `8f6c32ce5a1d7a552f09124806b791316c798914`，src tree `959fd092bc56b6d97f9b570b3c49aa65622912ba`，普通 O3/default binary SHA256 `30fe829c9ccaaaef91f243b714f8d12707b45e39835823f55d77df14fd5e2b87`。Pi 固定为 `90d863f7c1183b965f099855715d6e88c7b8a52c`，DeepSeek 固定为 `687f282e8625c6b8d11ff58211a90524be2e4317`。

新旧 Rust 交替配对；Pi/DeepSeek 仅比较不同批次中位数之比，不能当成本轮配对或因果估计。Rust/Pi 的核心请求审计剔除了 tools 字段：Rust 7 个工具、Pi 4 个，功能与完成边界并非完全相同。两种 Rust 模式都 attach 并捕获 checkpoint，无 live 不等于无观察工作；发布的 cursor、重放、后续订阅及预算失败语义仍需要维护，不能按当前 subscriber 数直接跳过 publication。DeepSeek 的 exit/flush CPU 单列，缺失的完整 write_bytes 不填零。当前 fake-provider 的 25 个受版本控制输入在两份 Rust 提交相同，整棵树为 `e3ab753811925c6fc544509d27e0526f8794f2cc`；这不会追溯补齐旧外部基准当时没有记录的绑定。

`/proc` CPU 按 100 ticks/s 换算，累计端点粒度 10 ms；profiler 为 250 Hz，二者不同。完整 CPU/写入截至记录的 cleanup 采样点，包含初始化、历史准备、验证和此前清理，不是单 turn 或退出后的 wait4。CPU inclusive 栈互相重叠，不能相加当独占时间；总样本为零时比例不可用；某函数未采到不证明其零成本。每 turn 的 `settled_us` 在 `/proc`/footprint 采样之前记录，因此该采样不直接计入 turn wall，但累计进程 CPU 与测量窗口 profiler 包含它；保持原基准不变，不通过删除采样改善数字。WAL frame bytes 不等于进程 write_bytes 或设备 I/O；dbstat 端点页映射不是逐次写入的精确归属。RSS/PSS 是采样值。完整仓库 CI、浏览器 E2E 和 main 合并未执行；PR 保持 draft，已有 main 冲突未在本次性能改造中处理。

**证据归档与复算。** [最终证据目录](evidence/final-cost-closure-runtime-20261028/README.md)保留 **1,110 个 payload 文件**，压缩归档 **11,534,480 bytes**，SHA256 `2b6fd27c79168b52844fdc24420380b63c7d09fdad3e25f6c1fa8d91f400f9fd`。从全新目录解压后，[离线验证](evidence/final-cost-closure-runtime-20261028/portable-verification.json)校验全部文件哈希，并独立重算正式比较、20 个诊断以及报告数值，全部通过；未运行产品或构建。

首批候选另有 1,214 个 payload 文件、12,180,536 bytes 归档，SHA256 `b08a058432d34d29c64c7d820a2a4f1cd994a96278ee47fc5b9b5dcf2e891864`，自身完整解压复算也通过。最终归档验证首批文件、身份、归档、manifest、复算凭据和固定参考的绑定；离线最终重放不再次读取独立首批归档本体，该边界在凭据中明确标记。两批正负结果、失败队列与旧 Pi/DeepSeek pin 均保留，没有用最终批次替换首批。

