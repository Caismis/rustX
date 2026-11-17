# rustX 每轮成本随历史增长：HEAD 基线与结构归因

日期：2026-11-10。任务 A；仅增加 benchmark、证据及本报告，**没有修改 `src/`，没有 commit 或 push**。测量 HEAD 为 `63207416a1d3a87ffc870d52cd5aeb549d057896`，schema 106，App Server v78。这比前序 schema-102 报告更新，不能把旧 ledger 的比例直接套到本次 HEAD。

当前版本没有达到每轮成本平坦的目标。最明确的两个根因是：

1. **工具关联属于历史事实，却被展开成每个请求私有的完整 JSON 数组。** 每轮两个请求反复写入所有旧关联，形成每轮 Θ(H)、累计 Θ(H²) 的存储与编码工作。
2. **一个声称有界的消息头读取实际扫描全部同类历史并排序。** `LIMIT 8` 限制结果数量，没有限制 SQLite 工作量。当前 bundled SQLite 的 VM steps 已复现这个问题。

此外有全局 B-tree 的维护、持久化输入树每次追加的新路径、未压缩活动上下文的驻留和 provider 全量请求。它们具有不同复杂度，不能都称为“完整历史重写”，也不能用更多 serializer 微调一起消除。

## 可复现基准及边界

```sh
python3 benchmarks/runtime/turn-growth.py \
  --output docs/pi-durable-cost-model/evidence/turn-growth-20261110/new-run
```

默认在现有 `target/` 中执行 `cargo build --release --locked --bin rustx`，然后依次运行两个独立场景，每个场景只有一个 session、一个持续驻留的热服务进程、512 轮、32 轮一个窗口。可用 `--turns`、`--window`、`--scenarios`、`--settle`、`--affinity` 配置。已有 release 时可加 `--skip-build`。需要 Linux `/proc` 和已安装的 Python `websockets`；fake provider 没有运行时依赖。

脚本复用 `app-server-capacity.py` 的认证、JSON-RPC client、terminal 检测和 native idle 检查，读取当前 Rust 协议常量并实际完成 v78 握手。旧 capacity 脚本自身仍写死 v76，其旧配置也将 `protocol` 放在 model 下；新脚本使用当前 provider 配置形式，没有修改旧脚本或添加协议兼容路径。

- 固定输入为 `turn growth`，固定最终回复为 `ok`，fixture usage 固定为 input 16/output 1。工具场景每轮先请求一次真正的内建 `read`，读取固定的 `fixture.txt`，再回复 `ok`，所以每轮恰好两个 provider 请求。call ID 长度固定；usage 是控制变量，不是真实模型 token 计费。
- provider 使用仓库 HTTP/SSE codec，逐个验证历史中 user/tool 消息的完整数量及固定内容，并记录每次 wire body 的长度、SHA-256。客户端和 provider 是服务进程的兄弟进程，不计入进程 CPU、内存、I/O。
- 工作区在仓库外的独立临时目录，输出位于指定 evidence 目录。显式 session name 排除自动标题请求；context window 为 1,000,000，避免此有限实验触发压缩。报告不声称测过真实网络延迟、真实模型推理或压缩任务。
- affinity 为 CPU 2、8，Tokio workers 为 4。窗口末等待 250 ms，读取目标 PID 的 `utime+stime`、`smaps_rollup` RSS/PSS、`io.wchar`/`write_bytes` 和全部 SQLite main/WAL 文件大小。CPU tick 为 10 ms，以 32 轮汇总；CPU/I/O 包含退休检查及 settle，逐轮 wall latency 截止 subscribed `attempt_settled`，另存到 native idle 的延迟。wall 是端到端量，仍包含 provider 解析与传输、客户端调度；只有资源计数严格限定 server PID。
- 32/128/512 轮时，用 SQLite backup 取得一致性副本，由另一个新服务进程 resume/checkpoint/subscribe。热 PID 不重启；复制、冷测和清理在窗口之外。**cold 指进程冷启动，OS 文件缓存仍热**；启动到监听完成的固定成本不包含在 attach 计时内。PSS 也受同时驻留热进程的共享代码页影响。
- `wchar` 包含网络 `write`/`writev`；它不是纯磁盘写入。`write_bytes` 是内核归属的存储写入量，也不是 SSD 内部写放大。SQLite 文件长度是留存量，不是累计写流量。

release 构建用时 5m23s；二进制 SHA-256 为 `5dfc1a26ad58692ccd96a275422967a78f62fb44a6ba20970dc3017e8fbeff06`。完整身份及检查见 [build-and-checks.json](evidence/turn-growth-20261110/build-and-checks.json)。

## HEAD 数字

每个场景所有 16 个窗口及每轮延迟均保留，没有删除慢样本：[完整可读表](evidence/turn-growth-20261110/baseline-table.txt)、[JSON](evidence/turn-growth-20261110/baseline/result.json)。以下为首、末窗口：

| 场景 / 轮次 | CPU ms/轮 | wall ms/轮 | RSS / PSS MiB | wchar KiB/轮 | write_bytes KiB/轮 | main / WAL MiB |
|---|---:|---:|---:|---:|---:|---:|
| text 1–32 | 11.56 | 11.99 | 105.69 / 103.46 | 532.56 | 545.50 | 1.98 / 2.13 |
| text 481–512 | 15.94 | 18.44 | 115.66 / 113.43 | 827.70 | 875.88 | 25.71 / 2.18 |
| tool 1–32 | 31.88 | 44.52 | 114.87 / 112.65 | 1378.97 | 1452.50 | 4.38 / 2.12 |
| tool 481–512 | 66.56 | 70.30 | 133.81 / 131.59 | 5476.00 | 5795.38 | 188.27 / 2.59 |

下表使用所有窗口的 midpoint 做含截距 OLS；斜率表示多一轮历史带来的变化。它描述这个有限区间，不能证明所有项都渐近线性。

| 指标的斜率 | text | R² | tool | R² |
|---|---:|---:|---:|---:|
| CPU µs/轮/历史轮 | 16.66 | 0.599 | 66.16 | 0.979 |
| wall µs/轮/历史轮 | 72.19 | 0.316 | 58.55 | 0.866 |
| wchar B/轮/历史轮 | 508.56 | 0.749 | 8468.85 | 0.997 |
| write_bytes B/轮/历史轮 | 560.74 | 0.653 | 8898.46 | 0.993 |
| 窗口末 RSS/PSS KiB/历史轮 | 15.05 | 0.765 | 42.19 | 0.978 |
| 窗口末 main KiB/历史轮 | 50.80 | 1.000 | 392.44 | 0.970 |
| 窗口末 WAL B/历史轮 | 98.84 | 0.624 | 950.97 | 0.931 |

text 的第 353–448 轮存在明显慢窗口，wall 均值约 50–68 ms；未过滤。它使 wall 的 OLS 斜率远大于首末差所暗示的趋势。单次运行无法将这些突发延迟完全归因于产品或宿主机，所以 **text latency slope 的精确值不稳健**。工具场景的 CPU/写入趋势、原生数据增长和 SQL 工作量证据更明确。没有把诊断运行的慢速计时当成普通基线。

冷测如下，`总 CPU` 为 attach 至 subscribe 后 settle 的进程 CPU 差：

| 场景 | 历史轮 | attach ms | checkpoint+subscribe ms | 总 CPU ms | RSS / PSS 增量 MiB |
|---|---:|---:|---:|---:|---:|
| text | 32 | 32.83 | 12.33 | 50 | 28.27 / 18.19 |
| text | 128 | 39.00 | 5.75 | 50 | 29.99 / 20.63 |
| text | 512 | 69.90 | 4.76 | 80 | 33.38 / 23.84 |
| tool | 32 | 37.58 | 6.45 | 60 | 28.20 / 18.37 |
| tool | 128 | 54.45 | 12.45 | 70 | 32.07 / 21.52 |
| tool | 512 | 121.21 | 9.11 | 120 | 39.19 / 28.95 |

备份中 request 数量分别通过 text 32/128/512、tool 64/256/1024 的检查。冷测不是每轮热成本，其增长是另一项需要处理的加载成本。

## 归因方法与可信度

`perf record -e cpu-clock` 被当前 `perf_event_paranoid=4` 拒绝，失败输出保存在 [perf-attempt.txt](evidence/turn-growth-20261110/perf-attempt.txt)，没有修改内核权限。改用用户态 gperftools 2.16 `libprofiler.so`，500 Hz、`SIGUSR2` 控制采样，分别采样第 1–128 轮和第 385–512 轮。四份 full-symbol folded stacks、累计栈及原始 profile 均保留；源码 URL、hash 和构建参数见 [profiler-identity.json](evidence/turn-growth-20261110/profiler-identity.json)。release 没有 inline debug 展开，不能把每个 inlined serializer 的成本精确拆到源码行。

按最内层可识别 rustX 子系统做互斥分类后，工具场景高历史比低历史多约 **31.22 ms CPU/轮**：observation/read projection +11.23、tool association/correlation +3.94、provider adapter +3.84、native request metadata +3.64、input manifest/Source +1.53、selected input +0.84、event commit +0.73、request commit +0.55、context +0.36、statistics +0.06、canonical commit/read +0.05、其余/未分类 +4.44。text 的差约 +6.80 ms/轮，其中观察分类约 +4.25。它们是采样估计，含固定项波动，不是逐函数隔离实验。见 [cpu-attribution.json](evidence/turn-growth-20261110/cpu-attribution.json)。

另运行一次独立 `strace -f -ttt -yy -xx -e trace=write,writev,pwrite64,pwritev,fsync,fdatasync` 512 轮。用 WAL frame header 的页号和每窗口 `dbstat` 估算页归属，记录 WAL 与 main 的实际成功 `pwrite64` 返回字节，保留未知项。**这不是前序报告那种完整 WAL 内容重放认证**：窗口内被释放/复用的页仍可能错分；不能将表级估算解释为精确因果份额。它尤其会低估/混淆大 snapshot overflow 的归属。trace 不是 headline latency/CPU 基线。见 [write-attribution.json](evidence/turn-growth-20261110/write-attribution.json)。

诊断样本中的文件 `pwrite64` 斜率分解如下，单位为 B/轮/历史轮；它不含 provider/socket `writev`，也不等于普通样本的 `write_bytes`：

| 归属估算 | text | tool |
|---|---:|---:|
| RequestSnapshot / 索引 | 13.51 | 3640.25 |
| event 索引 | 148.91 | 1007.88 |
| event 行 | 3.96 | 817.78 |
| native input / Request Source | 58.96 | 485.62 |
| canonical / transcript / descriptor | 64.14 | 531.88 |
| content frontier / layout | 38.24 | 180.73 |
| Trace catalog | 10.53 | 45.23 |
| 其他已识别页 | 73.34 | 382.21 |
| 未归属页 | -0.22 | 542.52 |
| WAL framing | 2.69 | 53.67 |
| 合计 | 414.05 | 7687.78 |

原生逻辑字节和最终 dbstat 独立于上述页归属近似，见 [sql-attribution.json](evidence/turn-growth-20261110/sql-attribution.json)。`analyze.py` 可重算三种归因，实际 bundled SQLite 的查询探针及反事实也提供了源文件。

## 按结构影响排序的根因与修复建议

### 1. 完整 `tool_associations` 被绑定到每个请求：最严重的可避免写入增长

`src/model/occurrence.rs:145` 的 `freeze_associations` 遍历整个 `history.tool_associations`；`:169` 和 `:175` 为每个旧关联重新写入当前 `request_id`；`:186` 再安装 wire labels。`src/durable/sqlite/native_request.rs:93` 把整个数组写进 metadata，`:288` 每个请求重新编码。`src/durable/sqlite/request.rs:815` 保存它，`:817` 插入 `snapshot_json`。`src/durable/sqlite/journal.rs:974` 更新完成序号、其他 observation 字段更新还会触及同一个宽行，可能产生额外 overflow/page 写入；没有逐次隔离这些 UPDATE 的份额。

工具场景最后一个请求 metadata 为 275,114 B，其中 association JSON 为 273,768 B、512 个元素。1024 个请求累计 association JSON 为 **139,945,412 B**；metadata 为 141,321,946 B。每轮 metadata 的斜率 **1070.21 B/历史轮**，association 自身为 1070.19 B，R² 约 0.999999。最后的 `request_snapshots` 表本身约 **137.74 MiB**，整个相关组约占主库 73%；text 的 association 斜率为零。宽 JSON 的增长绝非 provider 发送上下文的必然结果。

页归属近似将约 47% 的工具文件写入斜率放在 snapshot 组；另有约 7% 的文件斜率无法归属，不能强行补给它。CPU 中 association/correlation 和 native metadata 合计约 +7.58 ms/轮，占采样增量约 24%，尚未包括所有相关 SQLite/校验工作。

**结构修复：** 将关联作为 request-ID-free 的不可变历史选择数据，按 bounded chunk 追加一次；每个 request 仅持有关联集合 root/range、选中输入 cut 和一次 `request_id` binding。坐标在读取时相对 request 解释。生命周期更新放到独立小行，避免更新带历史 payload 的宽行。普通 append 每次只提交新关联和小 binding，旧请求仍能重建原始关联。不要只给 JSON 换编码，也不要给旧 schema 添加兼容层。

**风险：高。** request-only insertion、Derived/Original、重复 wire ID、不同 replay origin、stored continuation、retry 和旧 cut 的坐标必须保持精确，需逐请求语义 oracle 和 rollback/损坏测试。

### 2. “有界消息头”查询的实际历史扫描：CPU 的直接根因

`src/durable/sqlite/observation.rs:197` 的查询按 ledger 范围取固定 head，却未保证使用 ledger 范围索引。实际 SQLite 3.53.2 选择 `sqlite_autoindex_observation_headers_2 (reference_kind=?)`，遍历该类消息并用 temporary B-tree 排序。`src/durable/sqlite/schema.rs:590` 已有合适的 `observation_headers_ledger_idx`。

| 历史轮 | text 原查询 VM steps | tool 原查询 VM steps | 指定已有 ledger 索引，两个场景 |
|---:|---:|---:|---:|
| 32 | 420 | 740 | 79 |
| 128 | 1380 | 2660 | 79 |
| 512 | 5220 | 10340 | 79 |

每次均返回 8 条；原查询各有一次 sort，反事实均没有 sort。探针只读一致性备份，链接 **本次 release 使用的 `libsqlite3.a`**，不是拿系统 SQLite 推断。见 [query-probe.json](evidence/turn-growth-20261110/query-probe.json)。

该函数 inclusive samples 从 text 12→152、tool 153→656；500 Hz、各 128 轮对应约 **+2.19 / +7.86 ms/轮**，分别约占采样 CPU 增量 32%/25%。栈中可见 `pread64`、page-cache 查找和 SQLite 执行。它被 canonical publication receipt 与观察读取调用，不仅影响显式用户翻历史。

**结构修复：** 让所有固定窗口读取的访问路径成为代码契约：此处使用现有范围索引、保证按 ledger keyset 读取，并用跨数量级的 VM-step/query-plan 测试约束；同类查询按 cut/range/owner 逐一审计。无需增加索引或每轮 `ANALYZE`。**风险：低到中。** 保持 missing-header、损坏、旧 cut 及连续位置验证；反事实只证明这个 SQL 的工作量变平，尚未证明端到端收益。

### 3. 全局事件索引、canonical/content/Trace 索引：写入放大与缓存工作集增长

`src/durable/sqlite/schema.rs:475` 定义 scoped/native 索引，`:489` 为全局 kind 索引，`:948` 起为 observation/aggregate/transcript 等索引；`src/durable/sqlite/auxiliary/schema.rs:5` 定义独立 observation-kind 索引。HEAD 的两个 kind key **已经压缩**，见 `src/durable/sqlite/event_kind_key.rs:5`，不能再推荐一次同样的微调。

事件数每个 32 轮窗口严格为 text +256、tool +512；语义 append 数没有随历史增加。但索引树变大后插入搜索、页分裂、不同 kind/scoped leaf 的散写、checkpoint 回写仍增长。text 文件斜率中事件索引约占 36%，tool 约 13%；这些比例来自有复用歧义的页分类。canonical/transcript、content frontiers 和 catalog 的斜率也在写入分解中。它们主要是 B-tree 几何及写放大，不能据此宣称每轮扫描全部事件或按 H 重写 event JSON。

canonical 新事实入口为 `src/durable/sqlite/canonical/ledger.rs:322`，`:362` 插入新消息，`:368` 建 content/header 布局；event 和关联布局见 `src/durable/sqlite/journal.rs:907` 起。实际 CPU 样本的 canonical 组增量很小；普通 append 的旧身份扫描有 certificate，见 `src/durable/sqlite/append_validation.rs:171`。`:200` 的全量验证是 miss/cold/失效路径，不能误报成每个暖轮必扫。

**结构修复：** 保留 append-only canonical/journal 权威，按固定大小 sealed segment 隔离历史；热写入仅触及当前 segment 的小索引和增量布局，旧 segment 不再随新轮更新。内部 owner/row 使用短数值坐标，公共身份在边界绑定；保证查询通过 conversation/segment/range 路由，不以跨所有历史的通用索引维护换取每次写入便利。先审计哪些索引确有跨 segment 原子唯一性需求，再设计边界，不能直接删除所有索引。

**风险：高。** 跨 segment 唯一性、旧 cut、auxiliary 独立损坏处理、归档/分支和 recovery 都需保持；一个新的全局增长目录可能把成本移回原处。只缩短 key 不能满足严格平坦目标。

### 4. 输入 manifest 与 Request Source 的持久化路径：是 O(log H) 新节点，不是 O(H) body 重写

`src/model/selected_input.rs:421` 的持久 AVL join 为追加复制路径，`src/durable/sqlite/manifest.rs:257` 复用已提交旧节点，`:285` 递归保存新路径，`:290` 插入新行。Source 在 `src/durable/sqlite/observation_request/composed/nodes.rs:198` 保存相应 augmentation。

text 的 32 轮新增 native/Source 节点从第二窗口各 319 增至末窗口各 416；tool 从 703 增至 896。每组 native 与 Source 数相同。旧树确有共享，但深度增加使每次新提交路径更长，并维护不断增长的 Source page/hash 索引。该组约占 text 文件斜率 14%、tool 6%；工具 CPU 分类约 +1.53 ms/轮。

**结构修复：** 普通连续 append 的选择用不可变 bounded chunks 加 frontier/range binding 表示，旧前缀不为每个新 root 再建 AVL 路径；需要重排/压缩时建立新的有限选择段。Source 与 native selection 共用 chunk identity，保持可验证局部范围，不为旧内容重复建立另一套增长路径。**风险：中高。** digest、字节坐标、原始数字/Unicode、旧请求重建、精确 cut 和事务提交后才认证的规则都要保留。

### 5. provider 请求组装与 wire correlation：不可避免的发送量叠加可避免的重复遍历

`src/agent/execution/request.rs:619` 取得共享 history，`:663` 组装 selected input；它没有每轮重新加载所有旧 body。adapter 仍在 `src/model/adapter/validation.rs:86` 全遍历模态、`:98` 数图片；`src/model/adapter/openai/chat_completions.rs:1541` 分配随输入数量增长的 wire-message Vec，`:1554` 遍历所有消息；`src/model/adapter/validation.rs:223` 生成完整连续 body。correlation 在 `src/model/adapter/correlation.rs:100` 枚举历史工具、`:138` 构建排序/映射结构，`:34` 和 `:58` 两遍访问 wire fields。它是若干 O(C) 或 O(T log T) 遍历，不是固定成本。

文本请求 wire body 从 9,857 增至 74,243 B，工具最终请求为 208,387 B；每轮 wire body 的实测斜率分别为 126/776 B/历史轮，解释了 `wchar` 与文件 `pwrite64` 斜率的大部分差异。provider adapter 的采样 CPU 增量约 text +0.59、tool +3.84 ms/轮；工具关联另按第 1 项计。这里的 `C` 是实际选中上下文，`T` 是其中工具数。

**结构修复：** 在 immutable selected chunks 上保存模态、图片数量、tool correlation 验证及 adapter 编码段；只验证/翻译新段及边界，按 frozen invocation/replay policy 使证书失效。使用可计算 Content-Length 的共享编码段形成请求，避免每次构造完整 wire Vec 和连续 body 副本。仍发送全量上下文时，socket 字节量及传输 CPU 不会成为 O(1)。**风险：中高。** adapter overlay、ID 冲突、跨段 pending calls 和 continuation 语义必须精确，不能把 digest 相等当作全部授权。

### 6. 活动会话驻留和冷恢复：活动上下文未压缩时仍随 H 增长

`src/conversation/ledger.rs:88` 保留 canonical `Vec<Arc<MessageBlock>>` 和 ID map；`src/conversation/surface.rs:230` 保留 active selection、base IDs 和 ops。Ledger 与 Surface 共享 allocation，并非两个完整 body 副本。但 selected model input 另有 `src/model/selected_input.rs:130` 的树、`:133` 的 actual body；`src/durable/sqlite/projection.rs:1211` 向 manifest 安装内容，`:1254`/`:1271` 保留历史关联及 checkpoint items。

冷恢复在 `src/durable/sqlite/projection.rs:1376` 编译完整选中 history；`src/runtime/conversation_runtime/bootstrap.rs:1072` 再取该历史，`:1078` clone canonical body 构造运行态。热 history 在 `src/durable/sqlite/projection.rs:1161` 有增量缓存，不应把 cold compile 的全量工作算作每轮重做。

本次热 PSS 从首到末窗口增加 text 9.98 MiB、tool 18.94 MiB；冷载入增量亦随历史增加。**没有 heap ownership profiler，无法诚实地给 Ledger、selected input、SQLite cache、allocator 和 replay ring 分别填写内存百分比。** 源码证明这些 owner 的增长，RSS/PSS 只给总量；不能把所有 PSS 斜率归给一个 Vec。`max_resident_runtimes` 限制会话数，不限制单会话上下文；idle cache 回收 `src/durable/sqlite/idle_cache.rs:39` 不会处理持续活跃会话的这些 owner。

**结构修复：** immutable canonical body 使用一个内容 owner；提交后旧 body 可退为验证过的 SQLite location/reference，按需求解码，热缓存按字节预算淘汰。selected input 保存选择与摘要元数据，不永久保留所有 actual body；cold resume 只加载有限热尾和选择 frontier，其余按需读。**风险：高。** cache 淘汰不得使旧请求、取消中读取、分支及内容完整性验证失去正确性；必须测 cold/read latency，不能只看 RSS。

### 7. observation/projection/replay 与 transcript/descriptor 发布：已有界限，避免错误归罪

`src/runtime_client/checkpoint.rs:23` 的 native head 为 8 条；`src/runtime_client/projection/canonical.rs:165` 淘汰旧 descriptor。snapshot 使用 `src/runtime_client/projection/shared_snapshot.rs:25` 的 COW domain。`src/runtime_client/projection/publication.rs:55` 捕获 finite checkpoint metadata，并不意味着每个 native checkpoint 都把全会话历史序列化。

ring 上限为 4096 records 和 16 MiB **计费字节**（`src/runtime_client/projection.rs:178`、`:180`）；`src/runtime_client/projection/publication.rs:807` 淘汰前缀。`src/runtime_client/projection/subscription.rs:115` 通过 cursor 算偏移后 `VecDeque::get`，没有从 ring 起点扫描。故 ring 在填满前有留存增长，其后有界；它不能解释无界 Θ(H) 每轮成本。源码界限也不等于整个 RSS 精确小于 16 MiB。

观察分类的 CPU 增量较大，已确认的历史项是第 2 项底层 SQL；还包括缓存命中/页驻留、其他观察布局和固定工作波动。剩余量没有全部隔离，不能据样本编造一个“全历史 transcript 广播”机制。

**结构修复：** 保留有限 head、共享 domain、直接 cursor 索引及显式 resync；把有限结果同时落实为有限 SQL 工作量。最新 presentation 与审计历史继续分离，ended attempt/tool display 在 native 允许边界释放。若缩短 ring，需重新明确重连服务窗口。**风险：中。** 错删旧 cut/read capability 或把网络背压变成持久化语义变化会破坏订阅协议。

### 8. Request Trace catalog：每个请求重复定义，主要是固定负担及索引增长

`src/durable/sqlite/observation_trace/inspection.rs:27` 为每个请求写 catalog，`:31`、`:33` 再写 7 个定义绑定；`src/durable/sqlite/request.rs:527` 调用它。每窗口 text +32 catalog/+224 definitions，tool +64/+448，数量不随 H 增长。其文件斜率占比约 text 2.5%、tool 0.6%，不是本次工具 Θ(H²) 存储的主因。

**结构修复：** 一个 immutable、按配置版本绑定的共享定义集合，加每请求小 binding，保留独立可验证的原始 catalog 身份。**风险：中。** 不能从当前 tool registry 回填旧请求，也不能让一个共享损坏域抹掉独立事实。此项优先级低于前六项。

### 9. statistics folding：当前 HEAD 没有已证实的历史 fold 链

`src/durable/sqlite/observation_statistics/record.rs:1` 明确每个 relevant event 保存 bounded full state；`:125` 通过 indexed latest record 读取。`src/durable/sqlite/observation_statistics.rs:381` 复用 committed frame，`:389` 只应用当前 contribution。不是旧报告里可能讨论的稀疏 checkpoint 加无限 delta replay。采样增量 tool 约 +0.06 ms/轮，不能归因为线性重算统计。

它的记录、相关 event 索引及数值变宽仍有存储/树维护成本，已包含在 event 组，不可再次相加。**建议：** 保持此 bounded full-state/certificate 方案及旧 cut 测试；本任务没有证据支持重做统计体系。改变它的风险在于历史统计和事务认证失效，预期收益很低。

### 10. durable checkpoints、snapshots 与 context engine/compaction：区分三种“快照”

本次两个场景的 `projection_checkpoints` 都为 0：未执行压缩。每轮增长的是第 1 项 **RequestSnapshot**，不是每轮完整 durable context checkpoint。有限 Runtime Client checkpoint 则是第 7 项。

`src/context/engine.rs:36` 的 selected history clone 共享树；`src/context/tokens.rs:471` 使用树上的 visible-byte 汇总；`src/durable/sqlite/projection.rs:1171` 追加 history delta。没有证据表明这些已优化路径每轮重算全部 token 或完整历史哈希。`ObservedAnchor` 在 `src/context/tokens.rs:63` 共享 exact input root，也不是每次重建一份 ID 列表。

条件性增长仍存在：cold/epoch invalidation 进入 `compile_history`；已有 checkpoint 时依赖验证/编译遍历选中 items（`src/durable/sqlite/projection.rs:1393`）；压缩计划/summary 必须读取其处理的 span，`src/context/engine.rs:131` 的触发由 selected input 预算决定。活动上下文上限和对象界限只限制最坏规模，不能证明界限内每轮平坦。

**结构修复：** 有限热尾/selected chunks 与明确的压缩生命周期，按已经验证的 immutable cut 增量估算。summary/重建在必要边界付一次与其处理内容相称的成本；不把长期会话的全部旧 checkpoint/历史纳入每轮热路径。**风险：高。** 压缩会影响模型可见语义与 continuation；本实验没有压缩实测比例，不能给出它占当前斜率的非零百分比。

## 必须承认的下界与可执行优先级

令 H 为整个会话历史，C 为本次选中并发给 provider 的上下文。若要求 Chat Completions 每轮携带完整增长上下文，**wire bytes 为 Ω(C)，其传输工作不可能严格 O(1)**。已有请求长度实测验证了这个下界。累计审计存储随新增事实线性增长也是合理的；问题在于每轮重新付旧事实成本。PSS 的永久全历史驻留、每请求展开旧工具关联、消息头扫描/排序、旧索引被持续触及都可以避免。

应把目标写成：普通热轮的本地控制/持久化/观察工作只依赖本轮增量及固定热窗口；模型边界明确另计 O(C_wire)。若连 provider 边界也必须平坦，需要 bounded selected context 或受支持的 provider stored continuation；这是行为/协议选择，不能以不声明的丢历史实现。

建议顺序为：**先修固定 head 查询的访问路径契约；再移除 request 私有全量关联及宽行生命周期更新；然后重做普通 append 的 chunk/frontier 表示和历史冷驻留；最后处理 segment 索引与低优先级 catalog 重复。** 仅做第一项应有明确 CPU 收益，但不会解决工具快照写入；仅做 key 压缩也不会改变核心复杂度。

参考实现只读检查：pi-rust 的 `crates/rpi-harness/src/session/jsonl/cold_frames.rs:53` 定义验证 location，`:475` 保留 resident→cold 的值语义；Pi Durable 的 `packages/durable/src/harness/live.ts:67` 使用 latest history，`:87` 在结束时删除 generation/tools；`packages/durable/src/storage/sqlite/storage.ts:900` 在新 current-only base 后删除旧 revision。应借鉴 **历史事实存一次、热 presentation 有生命周期、旧 body 可冷读** 的边界，不能删除 rustX 承诺保留的审计旧 cut，也不能从这些参考推出全量 provider 发送在数学上为常数。两个参考仓库均未修改，revision 保存在 build identity 中。

## 检查与证据处置

普通两个场景、诊断两个场景、所有 provider request oracle、native ledger role/content、SQLite `integrity_check` 和三处 cold resume 均完成；Python 语法检查、OLS 已知直线自检和 `git diff --check` 通过。没有生产改动，因此没有再跑无关的完整 Rust suite；当前 release 确实由 HEAD 重新构建。

早期输出目录被当作仓库内工作区、一次 profiler/基线重叠、直接复制 main 文件忽略 WAL，以及重启热进程的迭代均没有作为正式基线。它们的表保留并在 evidence README 中说明。正式基线保留所有 32 轮窗口，并用一致性 SQLite backup 和独立 cold 进程。原始较大的 DB、strace 和 folded stacks 仅压缩归档，未以摘要替换原始证据。证据的限制是单次普通运行、采样误差、未做 heap owner census，以及页归属没有完整 WAL 认证；这些限制不影响完整关联 JSON 的 Θ(H²) 事实或 bundled SQLite 查询的线性 VM-step 证明。
