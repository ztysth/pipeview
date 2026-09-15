# 性能优化验证：2026-09-15

## 结论

前三项修改有明确正向收益，因此继续完成了 Span 名称共享、report 聚合、Konata 活跃阶段索引和空闲重绘优化。当前最终版本相对用户停止编辑时的快照，在真实大日志上解析耗时中位数下降约 19%，report 汇总下降约 66%，测量进程峰值 RSS 下降约 35%。

## 环境、输入与测量口径

- Intel Core Ultra 9 275HX，WSL2 Linux，Rust 1.93.1，默认 release 优化，依赖离线构建。
- 基础 Git 提交 `50cee75`。开始时 Cargo.toml、parser.rs、tui.rs、tests/performance.rs 已包含用户修改；最终实现保留这些修改。Cargo.lock 同步移除了已不再使用的 nom。
- 大日志 `examples/rancev_realtest.plog.zst`：解压 253659340 字节，1042326 条指令，7131750 个 span，1042322 个退休记录，0 个 E 记录。它是原有未跟踪输入，没有修改。
- 每个性能版本各运行三次，顺序为基线/优化/优化/基线/基线/优化，报告中位数；没有把这些三样本结果当作统计显著性结论。机器频率、文件缓存和分配器会影响耗时。
- 最终主表使用仓库中的 `examples/profile.rs`；基线编译相同驱动，仅替换 crate 名。该程序依次运行解压解析校验、默认 report、轻量 TUI 摘要、TraceView 构造、1024 个不同指令的详情查询。查询复用 TraceView，每条仅查一次，不使用 App 的 detail_cache。
- RSS 来自 `/usr/bin/time`，包括上述整个测量进程，不能直接视为交互 TUI 的精确峰值。进程墙钟还包括销毁数据的时间。
- 原始结果和源码指纹位于 [performance-data/2026-09-15](performance-data/2026-09-15/)。本机隔离快照和差异对照驱动保留在 `/tmp/pipeview-optimization-20260915`，临时目录不是永久 Git 历史。

## 前三项：独立确认收益

同机重跑原提交和开始测量时的用户快照，数据见 [before-results.txt](performance-data/2026-09-15/before-results.txt)。

| 指标 | 原提交 | 用户前三项修改 |
| --- | ---: | ---: |
| 解压、解析与校验，中位数 | 2657 ms | 1978 ms |
| App::new，中位数 | 785 ms | 784 ms |
| 测量进程峰值 RSS，中位数 | 1716712 KiB | 1892056 KiB |

- 解析耗时下降约 26%。
- 详情路径由全日志扫描变为复用索引：五个位置的重复查询由约 25–31 ms 降至 0.27–0.39 µs。这是重复访问同一位置的热数据测量，不能当作随机冷查询延迟。最终主表另外测量了不同指令的单次查询，约 3 µs。
- App::new 的总工作量仍然存在。源码确认它已在后台完成，UI 接收已准备的 App，避免在主线程构造全量摘要和索引；没有将这解释成 App 构造本身变快。
- 增加索引带来约 10% 的峰值 RSS 增长，后续名称共享优化覆盖了这部分代价。
- 测量开始后用户还有一次 push_line 的方法整理和 TUI 表达式格式调整。最终主表基线使用用户确认停止编辑后的精确源码快照，见 source-hashes.json 的 baseline_latest。

## 后四项：最终版本对比

原始数据见 [public-profile-results.txt](performance-data/2026-09-15/public-profile-results.txt)，计算结果见 [medians.json](performance-data/2026-09-15/medians.json)。

| 指标 | 用户停止编辑时的版本 | 最终版本 | 变化 |
| --- | ---: | ---: | ---: |
| 解压、解析、校验 | 2213 ms | 1791 ms | 耗时下降 19% |
| 默认 report 汇总 | 1220 ms | 417 ms | 耗时下降 66% |
| TUI 轻量摘要 | 115 ms | 66 ms | 耗时下降 43% |
| TraceView 构造 | 657 ms | 604 ms | 耗时下降 8% |
| 不同指令的单次详情查询均值 | 2.93 µs | 2.76 µs | 相近，样本范围重叠 |
| 测量进程墙钟 | 4.64 s | 3.07 s | 耗时下降 34% |
| 测量进程峰值 RSS | 1891860 KiB（1.80 GiB） | 1223460 KiB（1.17 GiB） | 下降 35%，约 653 MiB |
| 本平台 Span 结构大小 | 96 B | 64 B | 下降 33% |

私有 App/TestBackend 路径的辅助测量见 [after-results.txt](performance-data/2026-09-15/after-results.txt)。新旧 160×50 绘制均约 0.3 ms；共享名称增加一次间接访问，热详情查询可能有少量开销，因此没有宣称每条微观路径都变快。

### Span 存储

- `Span.stage`、`Span.lane` 改为 `SpanName`，内部使用薄指针 `Arc<String>`。解析器对名称去重，同一名称的记录共享存储。
- 每个句柄在本平台为一个指针；`Arc<str>` 是两个机器字，因此选择 `Arc<String>`。不使用全局泄漏的字符串池，也不引入 unsafe。
- 按字符串内容比较和哈希；支持跨 Trace 比较、独立持有 Span、后台线程传递。解析器或原 Trace 销毁后，克隆对象仍有效。
- Rust 库 API 的字段类型由 String 改为 SpanName。手工构造 Span 时使用 `stage: "IF".into()`、`lane: "main".into()`；读取可用 `.as_str()`，需要独立 String 时用 `.to_string()`。日志格式、CLI 报告和详情展示仍使用原名称。

### report 汇总

- 聚合期间借用字符串，只为不同的输出键分配 String。
- stage/lane 统计共用一次 span 扫描；stage_occupancy 从 stage_stats.total_cycles 派生。
- 首次 cycle 映射改为 HashMap，支持稀疏、乱序 instruction ID；报告输出仍由 BTreeMap 保持确定顺序。
- 实验性瓶颈聚合先按借用字段统计，再格式化不同键；保留名称带冒号时原有输出键碰撞的加和语义。

### Konata 活跃阶段索引

按 inst_id 保存活跃通道，只扫描目标指令的通道。E 记录仍需 stage 匹配；新 S 关闭同 lane 的旧 stage；R 关闭目标指令所有 lane；预览/EOF 关闭剩余阶段。

每个规模让全部指令先进入 IF 再统一退休，每个版本各三次。数据见 [konata-results.txt](performance-data/2026-09-15/konata-results.txt)。

| 同时活跃指令数 | 原实现中位数 | 最终实现中位数 |
| --- | ---: | ---: |
| 1000 | 1.327 ms | 0.528 ms |
| 2000 | 4.276 ms | 1.083 ms |
| 4000 | 13.906 ms | 2.107 ms |
| 8000 | 52.974 ms | 4.247 ms |
| 16000 | 213.535 ms | 9.460 ms |

16,000 条场景约快 22.6 倍，增长形态从接近平方变为接近线性。这是高活跃集合合成场景；普通 Konata 样例 2 单次解析仅从约 50 ms 到 47 ms，不能将 22.6 倍泛化到所有日志。

### 空闲重绘

全量模式在绘制后阻塞等待事件；预览模式仍每 100 ms 检查后台结果，但只有收到事件后重新绘制静态预览。没有预览时，加载计时继续低频更新。

伪终端中等待约 2.3 秒后退出，新旧 render 调用为 23 次和 2 次；最终版本的两次对应启动/加载切换。另验证了方向键、详情、Esc、信息面板、窗口缩放、退出和终端恢复。记录见 [pty-results.json](performance-data/2026-09-15/pty-results.json)。这是减少重绘次数的证据，没有据此虚构 CPU 百分比。

## 正确性验证

- 工作区最终实现：`cargo test --release --locked --offline`，50 项测试通过。
- `cargo clippy --release --locked --offline --all-targets -- -D warnings`、`cargo fmt --all -- --check`、`git diff --check` 通过。
- 新回归用例：名称共享及生命周期、前向声明、跨 Trace 比较、稀疏 ID、乱序 span、重复退休、无效退休延迟、冒号聚合键碰撞、Konata stage 匹配、多 lane、预览/EOF、负初始 cycle。
- 600 份固定种子生成日志（300 PLog + 300 Konata），逐一比较解析内容、两种选项下完整/TUI 摘要以及所有指令详情，一致。Konata 在旧版本中通过 HashMap 关闭阶段的输出顺序不稳定，比较原始 span 集合时规范化顺序；详情本身仍逐项比较。
- 两份经典 PLog、两份 Konata 和真实 PLog，共 5 个输入 × 实验性分析开/关，10 组 CLI report 输出逐字节一致。输出哈希见 [report-equivalence.json](performance-data/2026-09-15/report-equivalence.json)。

## 后续复测命令

```sh
cargo test --release --locked --offline
cargo clippy --release --locked --offline --all-targets -- -D warnings
cargo build --release --locked --offline --example profile
/usr/bin/time -f 'wall_s=%e rss_kib=%M' target/release/examples/profile examples/rancev_realtest.plog.zst
target/release/examples/profile --konata-scale
```

`profile` 使用与默认 CLI 相同的 512 MiB 解压输入上限；单次输出 JSON。测多个版本时先完成构建，再交替独立运行，避免让编译耗时或并行大内存任务污染测量。
