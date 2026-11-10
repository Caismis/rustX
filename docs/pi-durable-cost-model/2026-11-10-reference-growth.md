D：pi-rust / Pi Durable 绝对成本与历史增长实测（2026-11-10）

已完成独立驱动、三次完整复测、恢复与持久化校验。没有修改两个参考仓库的文件，没有修改 rustX 的 `src` 或 Agent A 的 `turn-growth.py`，没有 commit/push。新增代码在 `benchmarks/runtime/reference-growth/`；全部正式原始结果、进程计数器、会话存储和派生表在 `docs/pi-durable-cost-model/evidence/reference-growth-20261110/`。

最重要的修正是：本次公平的“保留并向无状态 Chat Completions 重传完整历史”负载下，两套参考实现都不能作为“端到端 CPU、驻留内存、write syscall 字节数全部严格恒定”的证据。pi-rust 的落盘写入近乎恒定，但 CPU、时延、RSS/PSS 明显增长；Pi Durable 的文本 CPU/时延较低，工具时延和驻留内存仍增长。应当把它们的持久化增量成本作为结构性标杆，同时单独核算不可避免的模型上下文准备与重传。

测量方法：每个产品、每个场景各三次 512-turn 单会话，共 6,144 个用户轮次、9,216 次 HTTP 请求、192 个 32-turn 窗口、36 次冷进程恢复。第二次反转产品及场景顺序，未剔除任何正式样本。下文数字是三次对应窗口指标的中位数；不是将三次样本池化后重算的分位数。完整 min/max、每次结果与所有 32-turn 行见 `summary.json` / `windows.csv`。4-turn 小样本先跑过所有路径和 2/4-turn 重开；正式结果不含该热身。

两边的实际工作相同：固定输入 `turn growth`、固定输出 `ok`、1,000,000 context window、4,096 输出上限。文本场景一次 SSE 响应；工具场景先请求一次原生 `read({"path":"fixture.txt"})`，得到相同的 18-byte `fixed tool result\n`，再响应 `ok`。使用 Agent A 同一个 Fake Provider codec/server；文本 SSE 四次编码/flush 的长度是 195 / 164 / 188 / 14 bytes，工具首请求是 329 / 170 / 188 / 14 bytes。所有产品、所有复测的响应 chunk 哈希完全相同，正文只含一个 `ok` 内容 delta，不设人为等待。驱动还逐次验证完整历史里的 user/assistant/tool 数量和正文，避免“忽略历史”获得假平坦。见 `provider.py:21`、`:37`、`:55` 与各 `provider.json`。

Pi 路径是真实 `Harness.open()` + `openNodeSqliteStorage()` + 生产 `openAICompletionsApi()` + `NodeExecutionEnv` + `CodingTools`；不是 `MemoryStorage`、faux provider 或测试调度器。`watchEvents` 消费真实提交事件，收到 `submission.wait()` 完成且消费 `run_end` 才给控制器 terminal（`pi-worker.mjs:34`、`:41`、`:77`）。pi-rust 路径是真实 CLI RPC，收到 `agent_end` 后的 `agent_settled(outcome=completed)`，校验流式正文及工具完成数量（`run.py:212`）。末态和每次恢复均读取完整持久化消息，Pi 正确翻页超过 1,000 条记录（`pi-worker.mjs:87`）；最终文本为 512 user + 512 assistant，工具为 512 user + 1,024 assistant + 512 toolResult。

宿主机是 i9-14900K / Linux / ext4 / NVMe；产品 `taskset -c 24-27`，provider `taskset -c 31`，控制器 `taskset -c 30`，`TOKIO_WORKER_THREADS=4`。24–27 是四个独立 E core，31 是另一个 E core。其他代理同时工作；affinity 不等于独占 CPU，也不能隔离共享 LLC、功耗、带宽、内核写回和 NVMe。因此给出复测范围，避免从单个 wall outlier 推导 CPU 结构。只统计产品 PID 的 utime+stime、smaps_rollup、io；provider 和控制器是兄弟进程，均排除。CPU 时钟粒度 10 ms，短恢复可能读到 0 ms，并不代表零 CPU。每窗口最后等待 0.25 s 让后台工作结算，CPU/I/O 包含该等待，逐轮 wall 不含；校验、关闭、重启单独核算。实现与 Agent A 字段对齐见 `run.py:53`、`:277` 和 README。

运行身份和持久化设置如下：

| 产品 | 源码/可执行文件及参数 | 持久化设置 |
| --- | --- | --- |
| pi-rust | revision `4056176bf4867f913099b101fd65b773895c99ba`；复用 `/home/capi/.cache/rustx-four-harness-20261109/pi-rust-target/release/rpi`，SHA-256 `4762ec079d86ce548d38a2840b2970ac6f8b093ada210692479b0c384b7a7017`；stock release，opt-level=3、thin LTO、codegen-units=16、strip=symbols，`default,clipboard` features、无 rustflags；原生 JSONL RPC。启动参数 `--mode rpc --offline --model fixture/turn-growth --thinking off --session-dir …`，恢复增加 `--session <jsonl>`；隔离 `RPI_CODING_AGENT_DIR`。 | 正常 JSONL + progress companion；`rpi-tools/src/os_env/append.rs:139` 把 append 放进一个 blocking dispatch，`:153` 为 `write_all`，`:154` 为 `flush`；没有添加或取消 fsync 选项。`rpi-tools/src/env.rs:148` 明示这是 visibility boundary，不是 power-loss fsync。不能把它与 SQLite NORMAL 宣称为相同掉电保证。 |
| Pi Durable | revision `1cedd32724abfcb0915f76cc61b6827e2c16dbad`；直接读取 `/home/capi/Documents/codes/pi` 的源码，Node `v24.22.0`，`node --conditions=source --import pi-imports.mjs pi-worker.mjs …`；`stream.maxRetries=0,cacheRetention=none`，thinking off。依赖复用 `pi-1ced/node_modules`；OpenAI 6.40.0、typebox 1.3.27、diff 8.0.4。node SHA、完整 argv、manifest/lock/source tree 哈希均在 identity/provenance。 | Node SQLite 3.53.4，WAL、synchronous=NORMAL(1)、page_size=4096、wal_autocheckpoint=1000、busy_timeout=5000 ms，关闭执行 TRUNCATE checkpoint。未改任何存储默认值；独立运行原生 adapter 查询见 `storage-config.json`。源码 `packages/durable/src/storage/sqlite/node.ts:179`、`:161`。 |

Pi 的源码 package.json 写着较新 OpenAI/HTTPS proxy 版本范围，但该 revision 的受跟踪 package-lock 和现有安装都记录 OpenAI 6.40.0、HTTPS proxy 7.0.6；两份 lock 哈希相同。本报告明确使用实际锁定安装，没有声称运行 manifest 中较新的版本。`pi-imports.mjs` 只做原源码/现有依赖的 module resolution，没有替换 provider、storage 或 runtime。Pi 采用受支持的 Node 源码启动；发布版预编译 JS 可有更小的冷启动成本，本文不把 346 ms 作为语言无关的存储恢复时间。

下表给出 1–32、97–128、481–512 三个窗口，单位均为 ms/用户轮次；所有中间窗口可直接从 CSV 对齐 rustX。

| 产品/场景 | 轮次窗口 | wall 平均 | wall p50 | wall p95 | 进程 CPU |
| --- | --- | ---: | ---: | ---: | ---: |
| pi-rust / text | 1–32 | 1.857 | 1.807 | 2.041 | 1.875 |
| pi-rust / text | 97–128 | 4.096 | 4.042 | 4.559 | 4.062 |
| pi-rust / text | 481–512 | 14.370 | 14.393 | 14.753 | 14.375 |
| pi-rust / tool | 1–32 | 4.297 | 4.223 | 5.983 | 4.062 |
| pi-rust / tool | 97–128 | 11.646 | 11.659 | 13.011 | 11.250 |
| pi-rust / tool | 481–512 | 50.161 | 40.197 | 50.763 | 36.250 |
| pi-durable / text | 1–32 | 7.778 | 4.326 | 9.590 | 10.938 |
| pi-durable / text | 97–128 | 4.220 | 3.880 | 6.806 | 5.938 |
| pi-durable / text | 481–512 | 5.729 | 4.933 | 9.091 | 5.312 |
| pi-durable / tool | 1–32 | 14.191 | 10.334 | 18.660 | 19.375 |
| pi-durable / tool | 97–128 | 11.523 | 10.458 | 15.787 | 15.312 |
| pi-durable / tool | 481–512 | 26.931 | 17.225 | 29.172 | 15.938 |

首窗口包含第一次 provider lazy import/JIT，32 和 128-turn 恢复后的第一窗口也包含重新热身。Pi 第 1/33/129 轮常有约 100–130 ms 的首次生成成本，不能把该成本藏到启动之外后再宣称每轮都只有 4 ms。Pi 工具末窗口平均明显高于 p50，另有调度/GC 长尾；保留了全部轮次。pi-rust 的增长也出现在 CPU，而非只有 wall，无法用共享机器噪声解释掉。

末窗口的写入与内存/会话大小如下。KiB=1024 bytes，MiB=1,048,576 bytes；“运行中存储”含现存 WAL/SHM，“关闭后”已完成 checkpoint/close，均排除 workspace、日志和证据文件。RSS/PSS 是驻留量，不是每轮分配量，也不等于 live heap。

| 产品/场景 | RSS MiB | PSS MiB：32→512 | wchar KiB/轮 | write_bytes KiB/轮 | 运行中存储 MiB | 关闭后 MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| pi-rust / text | 44.64 | 23.97 → 42.42 | 54.09 | 14.12 | 1.065 | 1.065 |
| pi-rust / tool | 74.77 | 27.20 → 72.61 | 341.36 | 24.38 | 2.207 | 2.207 |
| pi-durable / text | 247.39 | 107.28 → 195.38 | 264.99 | 225.12 | 4.765 | 0.773 |
| pi-durable / tool | 340.27 | 108.05 → 288.15 | 900.43 | 589.00 | 5.711 | 1.703 |

pi-rust 文本 `write_bytes` 从 14.00 到 14.12 KiB/轮，工具从 24.25 到 24.38 KiB/轮，落盘近乎平坦。Pi 文本从 216.25 到 225.12 KiB/轮（约 +4.1%），工具从 547.75 到 589.00 KiB/轮（约 +7.5%）；其 per-turn write 是低斜率，并非严格零斜率。`write_bytes` 是内核归因的存储写量，不是 SSD 实际流量；`wchar` 还含 socket/RPC write，不能用来指称磁盘写放大。Pi 512-turn 的关闭后持久状态仅约 0.773 MiB（文本）/1.703 MiB（工具），运行中的 4–6 MiB 大部分是可复用的 WAL 容量。关闭开销及 wait4 全进程 CPU/块写单独保存在 `closes.json`。

为观察斜率，先剔除每个新进程的首个 32-turn 窗口，再对余下窗口的 midpoint 做 OLS；每格是三次独立拟合斜率的中位数，每多 1 个历史轮次的边际变化。全部 16 个窗口的原始拟合、最后一进程内的拟合、R² 和各次斜率仍完整保留。负 CPU 斜率受热身/GC影响，不能解释成算法随历史变快。

| 产品/场景 | CPU ms/轮/历史轮 | wall ms/轮/历史轮 | wchar B/轮/历史轮 | write_bytes B/轮/历史轮 | PSS KiB/历史轮 |
| --- | ---: | ---: | ---: | ---: | ---: |
| pi-rust / text | +0.02590 | +0.02645 | +76.05 | +0.00 | +37.56 |
| pi-rust / tool | +0.06654 | +0.09008 | +630.03 | -0.07 | +94.77 |
| pi-durable / text | -0.00383 | +0.00233 | +94.72 | +18.74 | +190.13 |
| pi-durable / tool | -0.00249 | +0.02858 | +684.93 | +55.09 | +449.19 |

请求体的斜率在两个产品里完全相同：文本每增加一个历史轮次，下一轮 body 多 76 B；工具每个用户轮次有两个请求，合计多 630 B。pi-rust 的 wchar 斜率分别 +76.05/+630.03 B，减去 body 后只有约 +0.05/+0.03 B 的残余斜率。Pi 分别 +94.72/+684.93 B，减去 body 后约 +18.72/+54.93 B。残余仍含 HTTP headers/RPC 等，不能把它直接命名为纯数据库写量；但这足以证明原始 wchar 的线性项不能归咎于持久化重写。详见 summary 的 `provider_body_bytes_per_turn` 与 `wchar_minus_provider_body_bytes_per_turn`。

同一最后进程内（排除恢复后首窗口）pi-rust CPU 斜率三次都为正：文本 +0.02255～+0.02725 ms/历史轮，工具 +0.05975～+0.06623 ms/历史轮。它的选择/读取成本值得专门区分：`rpi-harness/src/session/jsonl/storage.rs:1583` 每次拿 context_plan 后调用 `read()`；`cold_frames.rs:1051` 把选中 payload 读回并重建 SessionContext，`:100` 读 range，`:107` 校验 SHA，`:114` parse；`rpi-ai/src/providers/openai_completions.rs:321` 又遍历全部 context messages，`providers/request.rs:41`/`:45` 编码完整 JSON。这些机制可以压低长期热 payload 驻留和追加成本，却没有把全历史 provider request 变成 O(1)。本次没有 CPU profile 来逐项归因，各阶段占比仍是源码解释而非实测百分比。

Pi 的增量读取也不等于所有数组恒定：`packages/durable/src/harness/context.ts:63` 复用 previous range、只扫描 added entries，但 `:95` 返回历史数组副本，`:154` 的 extendRange 仍构造增长的 entries/contributions/messages 数组。`packages/ai/src/api/openai-completions.ts:1228` 转换全量消息、`:1237` 遍历转换结果；observer mount 保留当前整个 transcript（`harness/view.ts:169`）。因此不能从 latest-only LiveDoc 推导整体内存常量。PSS 受 GC 和 allocator 高水位影响，本次不把驻留增长直接判为泄漏。

恢复数据的时间均从 fresh product process spawn 到“恢复并可用”的原生端点：pi-rust 回答 `get_state`，Pi 完成 open/root/watchEvents（并已读入 512-turn 的 1,025/2,049 entries 快照）。这里 OS 页缓存是热的，没有 drop_caches；不是断电/块设备冷读测试。表中 `32 / 128 / 512` 表示对应历史长度，各值单位如下。

| 产品/场景 | 恢复 wall ms：32 / 128 / 512 | 启动 CPU ms：32 / 128 / 512 | 恢复后 PSS MiB：512 | Pi 原生 open+watch ms：32 / 128 / 512 |
| --- | --- | --- | ---: | --- |
| pi-rust / text | 10.94 / 22.85 / 69.71 | 0 / 10 / 60 | 24.44 | — |
| pi-rust / tool | 16.44 / 43.99 / 143.68 | 10 / 30 / 140 | 29.67 | — |
| pi-durable / text | 346.19 / 345.55 / 346.15 | 520 / 510 / 520 | 67.97 | 10.22 / 10.90 / 12.60 |
| pi-durable / tool | 339.40 / 345.12 / 346.22 | 510 / 510 / 510 | 69.38 | 10.25 / 10.99 / 16.29 |

Pi 的总启动约 340–346 ms 多为 Node/源码模块启动，实际 open+watch 约 10–16 ms。首次恢复后的生成还会 lazy-load HTTP adapter/JIT；这笔成本计在第 33/129 轮和相应窗口，并在 `summary.reopen.first_post_reopen_turn_ms` 显式列出；512 后不再追加用户轮次。pi-rust 恢复随历史增长到文本约 70 ms / 工具约 144 ms，应独立纳入恢复 SLO。重开后的 PSS 远低于长期运行末窗口，说明当前驻留值包含可回收/高水位部分，不能用冷 PSS 替代热态容量指标。

公平性边界需要一起保留：

1. Pi 测的是生产 Harness 的本进程 watchEvents；自建适配器只向控制器输出简短 terminal，未运行 `packages/server` 的 WebSocket server。pi-rust 原生 RPC 则序列化全部事件；rustX App Server 还承担 WebSocket、checkpoint/subscribe、session catalog 和 replay 等功能。不能将三者总耗时当作完全等价实现的直接倍数。
2. 原生 system prompt/tool catalog 不同：Pi CodingTools 只有 read/write/edit/bash 四个，另外两个产品的目录更大。共同正文/工具输入/结果完全一致，但不是逐 byte 相同的请求 envelope。Pi 的每请求静态 body 较小；提供所有 request 长度与 hash 供核查。
3. rustX 的冻结请求、独立 Trace/Statistics 检查域、索引、历史 cut 与回放承诺，在这两个参考路径上没有一一对应且启用的产品面。Pi 也有真实任务/提交、精确 entry cutoff、存储与 observer；不能称它“没有持久化/没有观测”。
4. Pi `LiveDoc` 明确 `history: latest`，idle/tool boundary 做小 base（`harness/live.ts:63`），新 base 删除旧 current-only revisions（`storage/sqlite/storage.ts:899`）。这是减少临时呈现持久化的结构性生命周期规则。rustX 应按自己的历史承诺选择适用域，不能直接删除仍被承诺的历史。
5. Pi 的 WAL/NORMAL 与 rustX 同类，但 page_size/auto-checkpoint 字节阈值不同；pi-rust flush 没有同等掉电同步承诺。本次不调参数，不把落盘差异全解释成更高效的同一合同。
6. Agent A 当前默认 affinity 为 CPU 2/8（P core），本报告参考值使用 24–27（E core）；需要精确 wall/CPU 倍数时必须统一 core class，不能直接拿两个目录的数字相除。A 的资源区间还包含每轮 thread/read retirement 查询，本驱动在 terminal 后不另发 inspector/state 查询。I/O 字节对比不需要 CPU 频率归一化。

对 lead 的可执行标杆：在这个最小文本负载的长历史末窗口，Pi Durable 的稳态约 p50 4.93 / p95 9.09 ms、CPU 5.31 ms、write_bytes 225 KiB/轮；pi-rust 的写入底线约 14 KiB/轮，但 CPU 与长历史恢复并不更平坦。工具末窗口 Pi 约 p50 17.23 / p95 29.17 ms、CPU 15.94 ms、589 KiB/轮；pi-rust 约 p50 40.20 / p95 50.76 ms、CPU 36.25 ms、24.38 KiB/轮。两套参考各有优势，不能拿其中一套所有指标都当作“绝对平坦且更优”的统一目标。

结构性验收应把“持久化/可选观测附加成本”与“本轮必须构造/发送的模型历史”分账；在不改变全历史无状态协议的前提下，后者已有 +76/+630 B 的实测线性下界。对驻留量则区分历史索引/真实上下文、共享身体、暂态呈现和 allocator/GC 高水位。这里已经给出了可以直接与 Agent A 比较的绝对表与斜率，不建议再通过降低观测或掉电保证制造假改善。

复现主命令：

```sh
taskset -c 30 python3 benchmarks/runtime/reference-growth/run.py --output <new-run-1>
taskset -c 30 python3 benchmarks/runtime/reference-growth/run.py --engines pi-durable pi-rust --scenarios tool text --output <new-run-2>
taskset -c 30 python3 benchmarks/runtime/reference-growth/run.py --output <new-run-3>
python3 benchmarks/runtime/reference-growth/summarize.py --runs <new-run-1> <new-run-2> <new-run-3> --output <summary-dir>
python3 benchmarks/runtime/reference-growth/audit.py --runs <new-run-1> <new-run-2> <new-run-3> --output <summary-dir>/validation.json
```

证据入口：`README.md` 完整说明 schema、计时端点和单位；`identity.json`/`host-and-provenance.json` 记录精确身份与原构建 fingerprint；`summary.json`/`windows.csv` 为可重算汇总；`validation.json` 验证三次全部 12 个产品样本、全部 provider wire 哈希、每窗口算术、未修改的被测驱动 hash，以及六个 Pi 数据库 integrity_check。原始 `before/after.raw_proc`、逐轮延迟、provider 长度/正文 hash、reopen、close 和实际 session store 均保留。所有正式样本 accepted=true。
