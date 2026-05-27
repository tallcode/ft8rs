# TRY.md — FT8 流式解码尝试记录

本文只保留仍有复盘价值的尝试结论。长期技术状态、架构说明、CLI 用法放在
`STREAM.md` / `README.md`。

## 当前基线

- 默认编译路径：`rustfft@3840`。
- WSJT-X 对齐验证路径：`FFTW@3840`，通过 `--features fftw` 启用。
- `rustfft@4096` 已移除；运行时 FFT 引擎切换也已移除。
- 短测：`tests/ft8/210703_133430.wav`
  - 当前 `21` unique messages。
  - release 约 `3.3s`。
- 长测：`tests/ft8/230208_140300.wav`
  - 当前保护线 `422/449`。
  - 每段小于 `15s`，最慢观测约 `4.13s`。
  - timing residual：`baseline_drift - decoded_dt` median 约 `+0.785s`。
- 测试要求：
  - 性能/灵敏度测试只使用 release。
  - WSJT-X parity 验证使用 `--features fftw`。
  - 不通过降低 `ncand/ndepth`、关闭 AP、放宽门限或扩大非 WSJT-X 搜索来追分。

## 里程碑摘要

### Milestone 1: 361 -> 381

目标是先达到第一里程碑 `381/449`，并把 stream decoder 从一次性 full decode
改成 WSJT-X 风格的 `nzhsym=41/47/50` 渐进流。

关键突破：

- `sync8d` 时间索引不能环绕。
- WSJT-X `sync8d(cd0,i0,...)` 用 signed `i0`，越界 Costas block 贡献 0。
- Rust 旧实现用 `rem_euclid(NP2)`，把越界访问环绕到 `cd0` 尾部。
- 去掉环绕后长测提升到 `381/449`。

保留结论：

- FFT 表示切换不能替代 WSJT-X 对齐。
- residual/sbase/AP 输入结构修正即使单点不涨分，也应保留，因为它们减少架构偏差。

### Milestone 2: 381 -> 401

目标是达到第二里程碑 `400/449`。

有效对齐：

- `ft8_decode` pass 内 FFT 生命周期对齐：同一 pass 内 subtract 后不立刻重算 long FFT，下一 pass 再刷新。
- outer `syncmin`：depth 1/2 使用 `2.1`，depth 3 使用 `1.3`。
- `sync8` time-bin 按 Fortran 1-based 时间维访问，Rust 访问前转换为 `m-1`。
- AP `sync8d` 频率微调用 `ctwk * Costas`，second time refine 回到普通 Costas sync。
- CQ/AP 保存路径按 WSJT-X `split77/chkcall` 语义处理 `CQ_DX`、`CQ CALL GRID` 等形态。
- `pack_jt77::is_stdcall()` 修正 Fortran `iarea` 到 Rust 0-based 的转换。

关键突破：

- 弱 CQ miss 的主因之一是 `stdcall` 1-based/0-based 移植错误。
- 旧 Rust 把 `D1DX`、`F1PPH`、`R6KEE`、`IW1PUR` 误判为非标准呼号。
- `ft8_a7d` CQ `imsg=5` 因此生成 `CQ CALL`，没有生成 WSJT-X 的 `CQ CALL GRID`。
- 修正后长测达到 `401/449`。

撤回尝试：

- 临时把 candidate dt 改成 `(jpeak-1.5)*tstep` 虽能影响分数，但不是 WSJT-X 公式，已撤回。
- 不能通过放宽 `dmin/dmin2`、hard sync gate、syncmin 或候选搜索范围追分。

### Milestone 3: 401 -> 422

这一阶段主要清理 1-based/0-based、文件尾段和 diff 工具，最终保护线提升到 `422/449`。

有效修复：

- `pack77_1` 的 `R GRID` 判定修正为检查第三个词。
- `split77/chkcall` 标准呼号判定收紧，避免把 `RR73`、`73` 这类报告词当成标准呼号。
- `unpack77` 补齐 CQ invalid guard。
- stream AP memory 的 `chkcall` 镜像按 Fortran 赋值顺序处理第 2/3 位数字。
- `pack77_1` 拒绝 WSJT-X 不接受的两词 `CALL1 CALL2/R` 形态。
- `subtractft8` sample index 修正：
  - WSJT-X `nstart` 和 `j` 是 1-based sample index。
  - Rust `i=0` 应对应 Fortran `i=1`，因此 `j=nstart+rust_i`，访问 `dd0[j-1]`。
  - 旧实现第一轮实际访问 `dd0[nstart-2]`，比 WSJT-X 早 1 个 sample。
- 长文件测试从 `floor(duration/15s)` 改为 `ceil(duration/15s)`，EOF 非空尾段也解码。
  - 解决 `230208_140730` 片段大量假 miss。
- diff CSV 修正为稳定 6 列，并可通过 `FT8RS_WRITE_DIFF=1` 生成。
- diff 匹配把 `<CALL>` 归一为 `CALL`，但 `<...>` 保持原样。

重要线索：

- `230208_140300.wav` 的匹配消息显示稳定偏移：
  - `baseline_drift - decoded_dt` median 约 `+0.785s`。
- offset sweep 结果：

| offset | matched | residual median | 结论 |
|---:|---:|---:|---|
| `0.000` | `422/449` | `+0.785s` | 当前正式基线 |
| `0.250` | `424/449` | `+0.535s` | 诊断更好但非正式 |
| `0.500` | `426/449` | `+0.285s` | 匹配最高，但不应作为追分开关 |
| `0.785` | `418/449` | `+0.000s` | 证实偏移存在，但直接重切窗变差 |
| `1.000` | `412/449` | `-0.215s` | late large-drift miss 变多 |

结论：`0.785s` 偏移是真实诊断信号，但不能简单作为正式 slot offset。后续应继续对齐
WSJT-X 文件窗口、padding、连续缓冲和跨时隙 AP memory。

### Milestone 4: 稳定 422，准备目标 430

有效小对齐：

- baseline polynomial 阶数修正：
  - WSJT-X `nterms=5` 是 5 个系数，即 4 次多项式。
  - Rust 旧 `polyfit(..., 5)` 是 5 次多项式，已改为 degree 4。
- stream depth/AP 控制流：
  - `ndepth=1` 时，WSJT-X 在 `nzhsym<50` 直接返回；Rust stream 已跳过 41 阶段。
  - 外部 `ft8_a7d` 受 `lft8apon` 和 contest 6/7 约束。
- duplicate-gated subtract 假设已撤回：
  - 只看 `ft8_decode.f90` 外层容易误判 subtract 应在 duplicate check 之后。
  - 实际 WSJT-X regular subtract 在 `ft8b.f90` 内部 valid decode 后执行，然后才返回外层 duplicate check。
  - Rust “成功返回后立刻 subtract，再做 seen filter”符合有效执行顺序。

## 工程整理

### 模块边界

- 解码核心保持独立：
  - `src/ft8`: FT8/JT77/LDPC/AP/subtract/protocol/hashcallbook。
  - `src/stream`: slot 时间、slot 驱动、跨 slot session 状态。
  - `src/input`: WAV 文件、声卡采集、重采样入口。
  - `src/main.rs`: CLI 参数和输出，不承载解码逻辑。
- `util` 收口为 crate-internal，只保留真正跨层基础设施，目前主要是 FFT dispatch/engine。
- 单一 owner 的 util 已合并回 owner：
  - CRC -> `ft8::decode174_91`
  - AP LDPC helper -> `ft8::ap_decode`
  - pack/unpack/hash/subtract/protocol -> `ft8`

### CLI

- 文件：
  - `ft8rs file <wav>`
  - `ft8rs file <wav> --start-time YYMMDD_HHMMSS`
  - 文件名可推断时间戳时可省略 `--start-time`。
- 实时监听：
  - `ft8rs monitor` 列出输入设备。
  - `ft8rs monitor --device <index-or-full-name> [--slots N]` 监听输入。
- CLI stdout 只输出解码信息和 slot 完成分隔符；默认不输出 trace。

### 声卡 streaming

- 声卡主线程按系统 15 秒 slot 边界采集音频。
- `NativeSampleCollector` 保留 carry buffer，避免 audio chunk 跨 slot 时丢样本。
- decode worker 独占一个 `StreamDecodeSession`：
  - 保持 hashcallbook、AP memory、residual subtract 顺序和 `41/47/50` 状态顺序更新。
  - 主线程在采样等待期间轮询 decode event，使强信号 early decode 可以更早输出。
- 不并行 classic candidate/subtract loop，避免改变 WSJT-X residual 顺序语义。

## 性能尝试

### 有效

- AP downsample cache：
  - WSJT-X `ft8_downsample.f90` 用 `save x,cx` 保存长 FFT。
  - Rust 新增 `ApDownsampleCache`，AP 候选共享同一个 slot residual 的长 FFT。
- `gen_ft8wave` phase table cache：
  - 对齐 WSJT-X `ctab(0:NTAB-1)` 风格。
- 删除 `decode_from_f64` pass loop 前未使用 FFT。
- candidate workspace 从每个 candidate 新建改为每个 pass 复用。
- hard sync 统计避免重复计算。
- `compute_snr()` 和 `itone` 输出共用同一份 tone 序列。
- LDPC/OSD：
  - generator matrix 用 `OnceLock` 缓存。
  - OSD 内层 `mi/me/ce/e2/e2sub` 工作区复用。
  - `mrb_encode_into` 避免每次 encode 分配新 `Vec`。
  - `nextpat91` 去掉临时 `ms` 分配。
  - OSD reliability sort 改为 `sort_unstable_by`。
- 成功 decode 后 `message77` 使用 slice，不再 `to_vec()`。

当前性能结论：

- trace 显示主要瓶颈在 `ft8b -> try_decode_passes -> decode174_91` 的 LDPC/OSD。
- FFT、sync8、downsample 目前不是主耗时。
- WSJT-X classic FT8 主路径没有明显可直接照搬的 candidate 并行优化。
- `FFTW@3840` 与 `rustfft@3840` 的临时对比：
  - 短测均为 `21`。
  - 长测均为 `422/449`。
  - rustfft 在该轮长测更快，因此默认发布路径改为 `rustfft@3840`。
- 后续若继续优化，优先考虑 Rust 内部 LDPC/OSD workspace 复用；这属于内存模型优化，不是 WSJT-X 行为差异。

### 撤回或低收益

- pass 内 coarse downsample cache：
  - 因大块 `NFFT2` 复数数组 clone，长测变慢，已撤回。
- `sync8` 小数组 thread-local buffer：
  - 短测变慢，已撤回。
- 候选并行：
  - 可能改变 duplicate/subtract/residual 顺序，暂不做。
- FFTW wisdom / FFTW threads：
  - WSJT-X 构建支持相关能力，但当前 trace 中 FFT 占比很小，优先级低。

## 当前仍有价值的排查线索

- 剩余目标：第四里程碑 `430/449`。
- 继续优先源码架构差异，其次 miss 驱动；参数差异后置。
- 重点方向：
  - WSJT-X 文件窗口、padding、连续缓冲与 `0.785s` 起点偏移的关系。
  - `ft8_decode` 外层控制流、`ft8b` AP pass、`ft8_a7` same-parity memory。
  - 剩余 1-based/0-based：`maxloc/minloc`、`nint`、implicit integer assignment、Fortran array lower bound。
  - compound/hash/display 形态只用于 diff 诊断，不应被当成真实 miss。

## 最近验证

- `cargo check --tests` ✅
- `git diff --check` ✅
- `cargo test --release test_stream_decode_short_audio -- --nocapture` ✅
  - `21` unique messages
  - 约 `3.3s`
- `cargo test --release test_stream_decode_long_audio -- --nocapture` ✅
  - `422/449`
  - 每段均小于 `15s`
- `cargo test --release --features fftw test_stream_decode_long_audio -- --nocapture` ✅
  - `422/449`
  - 每段均小于 `15s`
- `target/release/ft8rs monitor --device "VB-Cable A" --slots 2` ✅
  - 声卡 live path 可正常按 slot 输出。
