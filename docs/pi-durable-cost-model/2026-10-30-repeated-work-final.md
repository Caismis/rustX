# 重复工作合并终版：实现、性能与可复算证据

此前确认的六类重复工作及排序缓存修正已实现并验证。**写入量下降，但延迟和 CPU 仍有明显回退，不能宣称整体加速，也没有追平 Pi。** 终版 60/60 对完整进程写入下降，各场景配对中位数减少 **0.732%–2.187%**；延迟四格中位数改善、八格回退。热请求中位耗时仍为固定 Pi 的 **4.879–5.538 倍**。这份结果不能作为“整体性能优化成功”的依据。

终版生产提交为 `43f56cfccc7d0b8b71dfb3ff0278ab0d2da06d22`，src tree 为 `82777e278d8980aafab098e76e1bd7c0aa4e8e7f`；普通 O3/default-feature 二进制 SHA-256 为 `fceee9f13a326cdb01472274edf8a186481fde18abd8956b5c4b07c66e2dda2d`。代码构建与功能检查完成于 10 月 29 日，最终测量在恢复任务后的 10 月 30 日（Asia/Shanghai）完成；证据目录沿用任务开始日期。

本轮 fresh baseline 始终为 `978ff5554f17b2214872addd6090ff4278143378`，src tree `aba7a9393cca1a7e990d778df27da78145b515bb`，二进制 SHA-256 `b6d283c8e478a79a3dc51740a00eb4c8f593d522d548ce3e4eb89a704f6caf14`。首批候选 `57304c804` 的数据独立保留，不充当终版基线、不与终版合并。没有首批候选对终版的直接配对消融，不能将两批数字变化因果归于排序修正。

**完成的改造。** 下表描述已由源码和回归证明的工作变化，不把操作次数直接换算成性能百分比。

| 改造 | 减少的重复工作 | 保留的功能与成本 |
|---|---|---|
| 请求几何遍历合并 | 新消息模板的边界查找、prefix/suffix 捕获从三遍合为一遍；额外 suffix 和首次 stable scalar 捕获从两遍合为一遍 | 原始字节、F/C 导航事实、坐标、排序、独立数值/深度准入、hash 和回滚保留；两片顺序使用同一 TEMP workspace |
| Metadata 计算与排序复用 | 按有序不可变 owner 复用预算、长度和排序；路径集合替代逐项线性查重；合并 frontier 遍历 | 保留重复项与首来源优先级、cut、fit/eviction 重算；每类仅缓存最新数组，无历史链 |
| Trace 最终 header 批量读取 | 已选出并截断的 1–32 条 header，由 N 次 SQL 合为一次 | 各 kind seek、洞检查、owner anchor 读取以及逐行校验仍执行，历史 cut 和错误顺序不变；整页不止一次 SQL |
| 成员捕获复用 | 同次 final settlement 的 canonical → terminal 命中时省一次 members 解码 | 仍查洞、读取实际最新 head/blob、精确比较字节；触发器改写、损坏、新 head 和回滚保留原处理，terminal 新值仍编码 |
| 紧凑记录正向预算校验 | 三个封闭 DTO 的正向证明通过时，跳过实际 JSON 数字/标点编码来计算长度 | 结构预检、typed decode、EOF 保留；false/未知仍计算实际 JSON 长度，不能因保守预算拒绝原本可读的密集浮点或宽整数；writer/attach 准入不变 |
| 事件索引合并 | 删除 `events_attempt_idx`，每条 attempt_id 非空的新事件少维护一个索引条目 | 共享 attempt-kind 索引；执行历史 11 个、lineage 6 个固定 kind 的有序流由 SQLite 合并，无全历史临时排序；分支启动成本仍在 |

首批复核发现，metadata 排序缓存额外保留一份 sorted Arc 数组，每次输出又复制引用；live 变化阶段也可能持续 miss。终版已将它改为**稳定排序置换与原地交换**：已排序输入的排序步骤不建置换、不重写输出；非平凡排序只复制 usize scratch 并线性交换，不复制 fragment Arc。三项新增回归覆盖 97 项长环、重复 key 的稳定顺序、缓存命中、live owner 连续变化、引用数量及缓冲区保持。prepare miss 仍保存一套原 owner 数组，不能称整个 metadata 流程零分配或零引用复制。

SQLite 仍为 **WAL/NORMAL**，schema **83 → 84**，不加入旧格式兼容或迁移。原生 kind 与 Trace kind 索引继续独立：原生 type 被有效改写后，Trace 仍须找到并验证自己独立保留的事实，不能因原生 kind 变化静默漏读。稳定 scalar 的完整身份仍覆盖 configuration owner 之外的 invocation/context/profile 等字段。

**正式测量。** 只新跑 Rust，六场景 × 两订阅模式 × 五对交错运行，共 120 个产品、60 对、1,410 次请求比较。恢复任务时重新校验源码、二进制及所有基准输入。正式运行前后未观察到 Cargo/rustc 进程，但这只是端点检查，不是持续独占机器的保证。

下表是每对百分比变化的中位数，wall 方括号为五对最小/最大值；不是两批中位数相除。wall 使用各产品测量轮次的中位延迟，完整 CPU/写入计数截至 cleanup 采样。

| Mode | Case | Paired median wall % [min,max] | Full CPU % | Full write bytes % | Sample anon PSS % | Sample RSS % |
|---|---|---:|---:|---:|---:|---:|
| no-live-subscriber | short-small | -0.107 [-14.197,80.414] | -3.053 | -2.176 | 0.539 | 0.190 |
| no-live-subscriber | short-large | 5.827 [-50.590,81.719] | 0.495 | -1.694 | -0.060 | 0.018 |
| no-live-subscriber | stream | 0.556 [0.133,4.058] | 7.143 | -0.732 | -0.352 | -0.377 |
| no-live-subscriber | large | 4.277 [-18.028,18.374] | 0.000 | -0.831 | -0.070 | -0.133 |
| no-live-subscriber | tool | -4.636 [-21.339,25.538] | 0.000 | -1.449 | 0.100 | 0.111 |
| no-live-subscriber | cancel | -4.619 [-18.993,8.161] | 0.000 | -0.857 | -0.027 | 0.147 |
| live-subscriber | short-small | 4.643 [-48.043,85.280] | 8.511 | -2.187 | -0.003 | -0.143 |
| live-subscriber | short-large | -2.324 [-5.029,5.791] | -0.833 | -1.669 | 0.476 | 0.169 |
| live-subscriber | stream | 0.096 [-0.112,2.861] | 3.571 | -0.732 | 0.251 | -0.181 |
| live-subscriber | large | 9.117 [-2.906,34.343] | 0.000 | -0.831 | 0.106 | -0.003 |
| live-subscriber | tool | 4.676 [-1.749,23.238] | 5.000 | -1.449 | -0.256 | 0.000 |
| live-subscriber | cancel | 7.077 [-9.336,19.320] | 7.143 | -0.857 | 0.421 | -0.157 |

完整写入 60/60 对下降，单对降幅为 **0.732%–2.725%**。延迟则为 **24 对下降、36 对上升**，单对范围 −50.590% 至 +85.280%。11 格五对范围跨零；no-live stream 是唯一五对全部回退的场景，范围 **+0.133% 至 +4.058%**，中位数 +0.556%。没有删除不利样本或用重跑替换它们。

完整 CPU 中位数两格下降、四格持平、六格增加，60 对为 18 降、13 同、29 升。测量窗口与完整生命周期不能混用：live 大正文、Tool、cancel 的窗口 CPU 中位数分别增加 **25%、14.286%、33.333%**；live 大正文的完整 CPU 中位数为 0%，不能据此宣称热路径没有退化。RSS 六格中位数下降、一格持平、五格增加；匿名 PSS 六降六升。所有指标、逆向配对及缺失比值见 [完整表格](2026-10-30-repeated-work-final-tables.md)。

| 热请求模式 | 历史轮数 | 窗口 CPU 配对变化 | 当前中位延迟 ms | 中位延迟 / 固定 Pi | 窗口 CPU / 固定 Pi | 完整写入 / 固定 Pi |
|---|---:|---:|---:|---:|---:|---:|
| no-live-subscriber | 8 | -4.545% | 15.296 | 4.879 | 2.391 | 4.056 |
| no-live-subscriber | 64 | +9.677% | 17.322 | 5.251 | 3.050 | 4.551 |
| live-subscriber | 8 | +7.042% | 17.230 | 5.038 | 3.125 | 4.055 |
| live-subscriber | 64 | -3.261% | 19.607 | 5.538 | 4.045 | 4.551 |

**WAL 与逻辑数据。** 四个独立 WAL 产品每格测量 32 轮。两版均为每轮 5 次主库写提交，本轮未继续减少事务数。

| 场景 | 写提交：旧 → 新 | WAL 帧：旧 → 新 | 帧数变化 |
|---|---:|---:|---:|
| short-small | 160 → 160 | 6,214 → 6,068 | -2.350% |
| short-large | 160 → 160 | 7,116 → 6,951 | -2.319% |

端点归属中，events 索引合计帧数分别为 **1,858→1,711 / 2,139→1,982**；删除的 `events_attempt_idx` 原对应 147 / 157 帧。导航页自动主键索引为 **81→82 / 174→166**；其余对象归属帧计数相同，包括 events 表 **288→288**，content 两表合计 **874→874 / 1,173→1,173**。端点归属不是每次写入瞬间的严格归因，也不能用来分配 CPU 或墙时收益。

列诊断中，两版导航页逻辑字节均为 **156,393 / 150,944**，新增行数均为 **229 / 224**。全主库逻辑值为 **1,121,675→1,121,740（+65）/ 1,147,654→1,147,622（−32）**，基本未变。此轮减少的是部分索引维护和重复计算，并未再次压缩请求正文。逻辑值、SQLite 页、WAL 帧、累计进程 write_bytes 和设备流量是不同指标，不能互换。

**CPU 诊断。** profile/control 的三类场景是 live 64 历史、no-live 64 历史和 no-live Tool。目录名里的 `large` 是 `short-large`，不是正式基准的大正文 `large`。下表是各自一次独立诊断，不替代正式五对测量。

| 场景 | Control 平均 ms/轮：旧 → 新 | Profile 平均 ms/轮：旧 → 新 | Profile 总样本：旧 → 新 |
|---|---:|---:|---:|
| live，64 历史 | 18.599 → 20.440 | 20.200 → 17.489 | 214 → 191 |
| no-live，64 历史 | 60.955 → 18.113 | 20.139 → 17.639 | 162 → 160 |
| no-live，Tool | 43.345 → 46.635 | 32.470 → 55.215 | 10 → 14 |

no-live 64 历史 baseline control 的 **60.955 ms** 均值完整保留（该产品中位数 18.943 ms、最大 304.132 ms），没有剔除慢轮次或拿 profiler 运行替换它。live 长历史 publication encode 的 inclusive 样本为 **31/214→31/191**，read_with_trace 为 **15/214→29/191**，请求 read-layout 为 **16/214→17/191**。这些栈会重叠，不能相加或当成每项修改的因果收益；采样频率为 250 Hz，Tool 只有 10/14 个样本。正式 `/proc` CPU 计数为 100 ticks/s，每个端点粒度 10 ms。

源码工作计数证明请求遍历和排序引用复制已减少，却没有证明整条路径更快。当前仍需要实际 checkpoint 输出、owned snapshot replacement/eviction、原生恢复事实、请求/Tool 来源校验，以及各可独立分页和损坏处理的派生域及索引。正向预算遍历本身也有成本。现有数据不足以将回退归因某一个组件，不能称作噪声，更不能把“尚未证明可省”写成“所有剩余成本都不可优化”。本轮已确认的具体重复项及复核发现的排序复制都已处理；整体性能差距仍然存在。

**固定参考。** 本轮未重跑 Pi 或 DeepSeek。Pi Durable 固定为 `90d863f7c1183b965f099855715d6e88c7b8a52c`（`packages/durable`），DeepSeek Harness 固定为 `687f282e8625c6b8d11ff58211a90524be2e4317`。Pi 比值是独立历史批次的中位数比，不能用前后两批比值变化代替 fresh Rust 配对效果。

相对固定 DeepSeek SDK idle，live short-small / short-large / stream / large 的平均延迟比为 **1.125 / 1.336 / 1.005 / 0.240**。DeepSeek idle 不等待最终 flush/fsync，也缺少相同的 partial-token 文本发布；退出/flush wait4 CPU 单列，完整进程写入不可得，不当作零。Rust/Pi 核心请求等价校验排除工具定义（7 对 4）；AppServer transport 不在此进程内基准中。

**验证与交付。** 终版 **2,273 项测试通过、0 失败、1 项既有忽略**，fmt 及全 target/feature Clippy（`-D warnings`）通过。15 步成功检查的 948 个 src/tests/Cargo 输入前后一致；本轮累计新增 22 项回归，201 项关键必测回归均被核对为实际通过。包含 72 项真实进程死亡、151 项 durable integration 和 62 项 process integration 测试。正确性使用 release 依赖/root O0/all features；性能来自单独干净的普通 O3/default-feature 构建。

首批的模块导入和 Clippy 失败日志保留在其证据中，修正后才进入构建准入。终版与首批各自都包含 120 个正式产品和 20 个独立诊断产品，均 strict accepted。首批 [报告](2026-10-29-repeated-work.md) 保留 live 大正文 +19.800%、live Tool +7.879% 等不利结果；它未被终版覆盖，终版也未证明排序修正解释了批次差异。

| 批次 | 生产提交 | payload 文件 | raw.tar.xz 字节 | SHA-256 |
|---|---|---:|---:|---|
| 首批 | 57304c8047fc49894aa502dbdda747c6ad6180f3 | 1,129 | 11,245,696 | bcaa813429c8dc564e9d032d42c328ecda2a3d8570c563af7d7ac82fd7ade090 |
| 终版 | 43f56cfccc7d0b8b71dfb3ff0278ab0d2da06d22 | 1,091 | 11,082,104 | 5fc04cd5de35f704fc3d2c06521b70dbcc37161e8caca1aad5d2710cb8f3f844 |

[首批证据](evidence/repeated-work-runtime-20261029/README.md) 与 [终版证据](evidence/repeated-work-final-runtime-20261029/README.md) 分别保留脚本、实际构建身份、原始产品、失败记录、固定参考 pins 和严格审计。两份归档均已在各自全新目录完成逐文件哈希验证，并独立重算正式结果、诊断及统计表；各自的 `portable-verification.json` 回执均为 accepted。复算不执行产品或编译器，每份回执只验证对应批次。

未运行全仓库 CI/browser E2E，也未合并 main；PR 仍有原有冲突并保持 draft。当前交付支持减少已确认的重复工作和写入，**不支持整体延迟或 CPU 已改善的结论**。
