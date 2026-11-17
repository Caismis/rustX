# 2026-11-10 turn-growth 证据

主报告：[2026-11-10-turn-growth-baseline.md](../../2026-11-10-turn-growth-baseline.md)。

正式基线为 `baseline/`。它的每个场景是一个持续驻留热 PID、512 轮；cold resume 由单独进程打开一致性副本。`identity.json` 包含 HEAD、binary hash、脚本/provider 输入 hash、protocol、affinity 和测量口径。`baseline-table.txt` 为未过滤 stdout；`baseline/result.json` 汇总全部窗口、OLS 和 cold 测量。各场景另有逐轮 latency、provider body 长度/hash、native 行数、dbstat 和进程原始采样值。

复现（从仓库根目录运行；输出目录必须不存在）：

```sh
python3 benchmarks/runtime/turn-growth.py --output /tmp/turn-growth-reproduction
```

所有大数据库和 runtime 文件原样归档到 `databases.tar.xz`，目录清单为 `database-archive-paths.txt`。`cut-32.sqlite`、`cut-128.sqlite`、`cut-512.sqlite` 是包含 WAL 最后提交的 SQLite backup，而非裸 main 文件复制。解包及重算：

```sh
cd docs/pi-durable-cost-model/evidence/turn-growth-20261110
tar -xJf databases.tar.xz
python3 analyze.py
```

`sql-attribution.json` 是 metadata/association 逻辑字节、dbstat 留存及节点增量；`cpu-attribution.json` 来自 full-symbol folded stacks；`write-attribution.json` 来自 `strace/` 的成功 `pwrite64`。后者利用窗口末 dbstat 匹配 WAL 页号，显式保留未知归属，不提供完整 WAL replay 认证，窗口内页复用可能错分。

`cpu-low128/` 的 1–128 轮与 `cpu-high128/` 的 385–512 轮各采样 128 轮，500 Hz。没有将它们的 latency/RSS 作为普通基线。相应 `*.folded.gz` 为完整符号栈；`*.cumulative.txt` 为 pprof 派生表，原始 `*.cpu.0` 在场景目录。采样器源码 URL/hash/编译选项见 `profiler-identity.json`。`perf-attempt.txt` 记录当前机器的权限拒绝；未改 sysctl。额外的旧 `cpu-low/` 32 轮试运行未用于正式归因。

`strace/` 使用以下命令（tracer 和 provider 的进程 CPU/写入不归入 server PID）：

```sh
python3 benchmarks/runtime/turn-growth.py --skip-build --turns 512 \
  --reopen --strace --output /tmp/turn-growth-strace-reproduction
```

无参数的 `--reopen` 禁用 cold 测量。原始 `server-1.strace.xz` 包含带时间、PID、FD path、128 B payload 前缀的 write/writev/pwrite64/pwritev/fsync/fdatasync 调用；WAL 的 24 B frame header 完整。未完成/恢复的 syscall 已配对，未匹配数为 0。较大的 strace 场景 `result.json`/`windows.json` 和顶层 `result.json` 使用 `.json.gz`；`analyze.py` 直接读取压缩文件。`wchar` 还包括 socket body，不能与文件写 payload 混为一谈。

`query-probe.c` 只读三个 history cut，用当前 release 的 SQLite archive 验证原始 head SQL 与强制现有 ledger 索引的 VM steps、sort 和 query plan。没有对 DB 执行 ANALYZE 或 DDL，没有修改产品 SQL。重建示例（archive 路径随 Cargo build hash 可变化）：

```sh
gcc -Wall -Wextra -Werror -O2 query-probe.c \
  ../../../../target/release/build/libsqlite3-sys-5391703acd79354d/out/libsqlite3.a \
  -lm -ldl -lpthread -o /tmp/turn-growth-query-probe
/tmp/turn-growth-query-probe baseline/tool/cut-512.sqlite
```

该命令需有 `sqlite3.h` 的 include 路径（本次使用 Cargo registry 中 `libsqlite3-sys-0.38.2/sqlite3`）。结果保存在 `query-probe.json`，实际 SQLite 版本为 3.53.2。系统 Python SQLite 仅用于逻辑/dbstat 分析，不用来替代这个 query-plan 验证。

排除的迭代只用于审计基准修正，均未混入正式 baseline：

- `invalid-repo-workspace-table.txt`：仓库内 workspace 使 native change recording 扫描 evidence/repo；工具轮约 500 MiB 写入受到污染。
- `invalid-overlap-table.txt`：与早期 profiler 短暂重叠，停止后重新跑基线。
- `before-backup-fix-table.txt`：曾裸拷 main；WAL 尚存时 cut 可能遗漏提交。
- `hot-restarts-table.txt`：曾重启热进程进行 cold 测量；不适合给持续驻留内存作斜率。本次正式实现改用单独进程。

原始失败迭代工作目录在 `/tmp/`，不属于被接受测量。正式原始输出、诊断原始输出、源码 probe、派生计算和 SHA-256 清单均在此目录；没有保留 profiler 的本机安装作为交付依赖。`checks.txt` 和 `build-and-checks.json` 记录验证。两份 reference 仓库仅只读，未构建或修改；本任务不提交或推送。
