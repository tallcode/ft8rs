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
  - 当前保护线 `424/449`。
  - 默认 slot 起点偏移 `+0.785s`。
  - timing residual median 约 `+0.000s`。
  - 每段小于 `15s`。
- 测试要求：
  - 性能/灵敏度测试只使用 release。
  - WSJT-X parity 验证使用 `--features fftw`。
  - 不通过降低 `ncand/ndepth`、关闭 AP、放宽门限或扩大非 WSJT-X 搜索来追分。

## 里程碑摘要

### Milestone 1: 361 -> 381

- 把 stream decoder 从一次性 full decode 改成 WSJT-X 风格的
  `nzhsym=41/47/50` 渐进流。
- 关键突破是 `sync8d` 时间索引不能环绕。WSJT-X 使用 signed `i0`，越界
  Costas block 贡献 0；Rust 旧实现用 `rem_euclid(NP2)`，把越界访问环绕到
  `cd0` 尾部。去掉环绕后长测达到 `381/449`。

### Milestone 2: 381 -> 401

- `ft8_decode` pass 内 FFT 生命周期对齐：同一 pass 内 subtract 后不立刻重算
  long FFT，下一 pass 再刷新。
- outer `syncmin`：depth 1/2 使用 `2.1`，depth 3 使用 `1.3`。
- `sync8` time-bin 按 Fortran 1-based 时间维访问，Rust 访问前转换为 `m-1`。
- AP `sync8d` 频率微调用 `ctwk * Costas`，second time refine 回到普通
  Costas sync。
- `pack_jt77::is_stdcall()` 修正 Fortran `iarea` 到 Rust 0-based 的转换。
  旧实现把 `D1DX`、`F1PPH`、`R6KEE`、`IW1PUR` 等误判为非标准呼号，导致
  弱 CQ AP 模板错误。修正后长测达到 `401/449`。

### Milestone 3: 401 -> 422

- 修正多个 1-based/0-based 与格式 gate：
  - `subtractft8` sample index：Rust `i=0` 对应 Fortran `i=1`，
    因此 `j=nstart+rust_i`，访问 `dd0[j-1]`。
  - `pack77_1` 的 `R GRID` 判定修正为检查第三个词。
  - `split77/chkcall` 标准呼号判定收紧，避免把 `RR73`、`73` 当成呼号。
  - `unpack77` 补齐 CQ invalid guard。
  - stream AP memory 的 `chkcall` 镜像按 Fortran 赋值顺序处理第 2/3 位数字。
- 长文件测试从 `floor(duration/15s)` 改为 `ceil(duration/15s)`，EOF 非空尾段也
  解码，解决 `230208_140730` 片段大量假 miss。
- diff CSV 修正为稳定 6 列；匹配时把 `<CALL>` 归一为 `CALL`，但 `<...>`
  保持原样。
- 录音起点诊断显示稳定偏移：`baseline_drift - decoded_dt` median 约
  `+0.785s`。

Offset sweep 仍作为诊断资料保留：

| offset | matched | residual median | 结论 |
|---:|---:|---:|---|
| `0.000` | `422/449` | `+0.785s` | 旧窗口，timing residual 未对齐 |
| `0.250` | `424/449` | `+0.535s` | 诊断更好但非正式 |
| `0.500` | `426/449` | `+0.285s` | 匹配最高，但不应作为追分开关 |
| `0.785` | `424/449` | `+0.000s` | 当前正式对齐窗口 |
| `1.000` | `412/449` | `-0.215s` | late large-drift miss 变多 |

结论：`0.785s` 偏移是真实诊断信号，当前先作为长测试对齐窗口。后续应继续
对齐 WSJT-X 文件窗口、padding、连续缓冲和跨时隙 AP memory。

### Milestone 4: 稳定 422，准备目标 430

- baseline polynomial 阶数修正：WSJT-X `nterms=5` 是 5 个系数，即 4 次多项式；
  Rust 旧 `polyfit(..., 5)` 是 5 次多项式。
- stream depth/AP 控制流：`ndepth=1` 时 WSJT-X 在 `nzhsym<50` 直接返回；
  Rust stream 已跳过 41 阶段。外部 `ft8_a7d` 受 `lft8apon` 和 contest 6/7
  约束。
- duplicate-gated subtract 假设已撤回：regular subtract 实际在 `ft8b.f90`
  内部 valid decode 后执行，然后才返回外层 duplicate check。Rust
  “成功返回后立刻 subtract，再做 seen filter”符合有效执行顺序。

## 工程整理

- 解码核心保持独立：
  - `src/ft8`: FT8/JT77/LDPC/AP/subtract/protocol/hashcallbook。
  - `src/stream`: slot 时间、slot 驱动、跨 slot session 状态。
  - `src/input`: WAV 文件、声卡采集、重采样入口。
  - `src/main.rs`: CLI 参数和输出，不承载解码逻辑。
- `util` 收口为 crate-internal，只保留真正跨层使用的基础设施，目前主要是
  FFT dispatch/engine。
- 单一 owner 的 util 已合并回 owner：
  - CRC -> `ft8::decode174_91`
  - AP LDPC helper -> `ft8::ap_decode`
  - pack/unpack/hash/subtract/protocol -> `ft8`
- CLI：
  - `ft8rs file <wav>`
  - `ft8rs file <wav> --start-time YYMMDD_HHMMSS`
  - `ft8rs monitor`
  - `ft8rs monitor --device <index-or-full-name> [--slots N]`
  - `ft8rs monitor --device <device> --udp --udp-host 127.0.0.1 --udp-port 2238`
- CLI stdout 只输出解码信息和 slot 完成分隔符；默认不输出 trace。

## 性能尝试

有效且保留：

- AP downsample cache：同一 slot residual 的 AP 候选共享长 FFT。
- `gen_ft8wave` phase table cache：对齐 WSJT-X `ctab(0:NTAB-1)`。
- 删除 `decode_from_f64` pass loop 前未使用 FFT。
- candidate workspace 从每个 candidate 新建改为每个 pass 复用。
- hard sync 统计避免重复计算。
- `compute_snr()` 和 `itone` 输出共用同一份 tone 序列。
- LDPC/OSD：
  - generator matrix 用 `OnceLock` 缓存。
  - OSD 内层工作区复用。
  - `mrb_encode_into` 避免每次 encode 分配新 `Vec`。
  - `nextpat91` 去掉临时 `ms` 分配。

撤回或低收益：

- pass 内 coarse downsample cache：大块 `NFFT2` 复数数组 clone 导致长测变慢。
- `sync8` 小数组 thread-local buffer：短测变慢。
- 候选并行：可能改变 duplicate/subtract/residual 顺序，暂不做。
- FFTW wisdom / FFTW threads：当前 trace 中 FFT 占比很小，优先级低。

当前性能结论：

- 主要瓶颈在 `ft8b -> try_decode_passes -> decode174_91` 的 LDPC/OSD。
- FFT、sync8、downsample 目前不是主耗时。
- WSJT-X classic FT8 主路径没有明显可直接照搬的 candidate 并行优化。

## 最近 WSJT-X 对齐

### Progressive Decode State

- WSJT-X 在 `nzhsym=50` 时继承 `ndecodes=ndec_early` 和 `allmessages`。
- ft8rs 新增 `DecodeOptions.initial_messages`，stream 在 full-stage 传入
  early messages，仅参与 duplicate/pass 控制，不作为本次返回结果。
- 这是架构对齐，但不是当前剩余 miss 的主因。
- WSJT-X disk early path 在最终 `nzhsym=50` 仍然执行
  `id2a(50*3456+1:)=0`。stream final stage 已同步补零最后 `7200` samples，
  避免把完整 15s buffer 当作最终输入。这是 offset 对齐后重新复核出的
  window/padding 类差异；RustFFT/FFTW 长测仍保持 `424/449`，但源码边界更干净。

### LDPC / OSD 数值同构

- `platanh` 改为 WSJT-X `platanh.f90` 的分段线性近似和 `±7.0` 饱和，而不是
  精确 `atanh`。
- OSD reliability ordering 改为本地 `indexx_ascending`，按 WSJT-X
  `indexx.f90` 生成升序索引，再反转用于 MRB。
- `decode174_91` 内部 BP/OSD 工作数组、`tanh/platanh`、posterior `zsave`
  和 OSD distance 累加改为 WSJT-X default `real` 形状。入口仍接收上层
  `f64` LLR，但进入 LDPC 后立即收窄到 `f32`，避免后续弱信号诊断被
  Rust 内部 f64 长链路干扰。
- `try_decode_passes` 改为 WSJT-X `cycle` 语义：CRC-good codeword 若因 all-zero、
  message type、unpack 或 contest quirk 不合法，不让整个候选立即失败，而是
  继续后续 pass。
- `is_valid_message_type` 修正为 WSJT-X 条件：
  `i3>5 .or. (i3==0 .and. n3>6)`。

### `sync8` / Baseline 排序同构

- WSJT-X `sync8.f90` 在 `red`/`red2` 40% percentile、candidate0 生成和 final
  candidate sync 排序中都使用 `indexx`。
- ft8rs 已将这些位置改为本地 `indexx_ascending`。FT8 spectrum baseline 的
  percentile selection 也改为同一 index-based 选择。
- near-dupe 边界保留 `tdiff < 0.04 - 1e-12`，用于模拟 WSJT-X 单精度边界，
  避免把刚好相差一个 `NSTEP` 的候选误合并。

### FFT / Downsample 数值路径

- FT8 core 调用改为 WSJT-X 同名同向 wrapper：
  - `four2a_r2c` 对应 `call four2a(x,n,1,-1,0)`。
  - `four2a_c2c(...,-1)` 对应 complex forward。
  - `four2a_c2c(...,1)` 对应 complex inverse。
- `ft8_downsample` 和 AP downsample 使用 unnormalized inverse FFT，再显式乘
  `fac=1/sqrt(NFFT1*NFFT2)`，不再用 normalized inverse 的数学等价写法。
- `ft8_downsample` 和 AP downsample 的 `i0/ib/it` bin 取整改为先收窄到
  Fortran default `real` 语义，再执行 `nint`。这不是追分参数，而是对齐
  `ft8_downsample.f90` 中 `df/baud/f0` 默认实数表达式的 rounding path。
- `cshift(c1,i0-ib)` 改为 signed shift + `rem_euclid`，避免极低频边界时
  `usize` 下溢。
- `ft8b` soft-symbol metric 链路按 WSJT-X 默认 `real/complex` 收窄：
  `csymb/1000`、`s2`、`imetric=2` square、`bm/den` 和
  `normalizebmet` 中间算术都走 f32 形状。此项保持 RustFFT/FFTW 长测
  `424/449`，属于后续 miss 诊断降噪，不是追分调参。
- `ft8b` 内部同步细化链路继续对齐：
  - Costas sync template 和 `delf` tweak template 用 default `real`
    `phi/dphi/cos/sin` 形状生成。
  - `sync8d` 的复数点积和功率累加收窄到 f32，匹配
    `sync8d.f90` default `complex/real`。
  - regular `s8` 改回 WSJT-X 的未缩放 `abs(csymb(1:8))`，`cs` 保持
    `csymb/1e3`；`xsnr2` 公式恢复源码的 `/3.0e6`，不再用等价补偿 `/3`。
  RustFFT/FFTW 正式窗口长测仍保持 `424/449`。
- `ft8_a7d` AP metric 同步做同构整理：`nsym=1` 改回 WSJT-X 的
  `abs(cs(graymap(...),ks))` 路径，而不是未缩放 `s8`；`cs/s2/bm/den`
  也收窄到默认 `real` 形状。RustFFT/FFTW 长测仍保持 `424/449`。
- 重新审视“旧 offset 下暂缓”的细节后，继续收窄 `scalefac*metric`、
  `apmag=maxval(abs(llrz))*1.1` 和 regular SNR `xsig/xnoi/xsnr/xsnr2` 的
  中间算术到 WSJT-X default `real`。在 `+0.785s` 窗口和当前细节叠加后，
  RustFFT/FFTW 长测仍保持 `424/449`。
- `ft8b` 和 `ft8_a7d` 初始 time-refine 入口的
  `nint((xdt+0.5)*fs2)` 改为先收窄到 WSJT-X default `real` 再取整。该项没有
  单独改变短测或长测结果，但消除了一个可能产生 1 个 200Hz sample 差异的
  rounding path。
- 在 `+0.785s` 正式窗口下重新复核旧的“数学等价但表达式不一致”项：
  - regular/AP downsample 的 `df/baud/f0/ft/fb` 表达式、taper 生成和
    `fac=1/sqrt(float(NFFT1)*NFFT2)` 都按 WSJT-X default `real` 路径落地。
  - AP `ft8_a7d` 的 `sync8d`/frequency tweak 也从 f64 累加改为
    `sync8d.f90` 同样的 default `complex/real` f32 形状。
  - RustFFT/FFTW 短测均保持 `21`，长测均保持 `424/449`；这是对齐降噪，
    不是灵敏度调参。
- `sync8`、FT8 spectrum baseline、regular `ft8b` `xbase` 和 stream AP memory
  `xbase` 的 bin selection 改用 WSJT-X default `real` 的 `nint` 路径；`xbase`
  的 `10**` 表达式也按 default `real` 收窄。RustFFT/FFTW 正式窗口长测仍保持
  `424/449`。
- 重新试过把 `sync8` 内部 `s/sync2d/red/candidate0` 全链路收窄到 f32，结果
  正式窗口长测降到 `423/449`，已撤回。结论：`sync8` 不能简单整体 f32 化，
  后续若继续对齐需更细地对照 FFT 输出缩放和 Fortran `real` 数组边界。
- 重新试过只把 `sync8` candidate 输出的 `freq/dt/sync` 收窄到 default
  `real`，正式窗口长测同样降到 `423/449`，已撤回。结论：这些旧 offset 下
  暂缓的 sync8 精度项在当前 offset 下仍不能直接保留，需要先找出
  `sync8` 前段 FFT/数组存储差异。

### `nuttal_window`

- 对照 WSJT-X `lib/nuttal_window.f90` 和 `lib/ft8/get_spectrum_baseline.f90`，
  FT8 spectrum baseline 使用的窗口常量为：
  - `a0=0.3635819`
  - `a1=-0.4891775`
  - `a2=0.1365995`
  - `a3=-0.0106411`
- ft8rs 之前使用的是另一组 Nuttall 常量。已改为 WSJT-X 常量和同号展开：
  `a0+a1*cos(x)+a2*cos(2x)+a3*cos(3x)`。
- 影响范围是 `savg/sbase/xbase`、SNR 和 false-positive gate。

### `subtractft8` / `gen_ft8wave`

- `subtractft8` sample index 对齐：`nstart` 和 `j` 按 Fortran 1-based sample
  index 映射。
- `subtractft8` `nstart=dt*12000+1+idt` 按 Fortran implicit integer assignment
  截断，而不是 round。
- LPF 改为 WSJT-X 结构：
  - `NFFT=NMAX=180000` circular FFT filter。
  - `cw(1:NFILT+1)=window/sumw`。
  - `cshift(cw,NFILT/2+1)`。
  - forward FFT 后 `cw=cw*fac`。
  - `cfilt` forward、乘 `cw`、inverse，再应用首尾 `endcorrection`。
- `gen_ft8wave` complex envelope 已对齐：
  - first ramp: `(1-cos(angle))/2`
  - last ramp: `(1+cos(angle))/2`
- `subtractft8.f90:sqf()` refined-DT 结构已对齐：
  - `-90/+90/0` trial offset 每次都从 `dd0` 重建局部 `dd`，完成一次 subtract，
    不污染输入 buffer。
  - `ldt=true` 时，对减除后的 `x` 做 `NFFT=180000` real FFT，只累加
    `f0-1.5*baud` 到 `f0+8.5*baud` 的信号频带能量。
  - `sqf` 返回值、band-energy 累加和 `peakup` 算术按 WSJT-X 默认
    `real`/`real*4` 路径收窄到 f32。
  - `peakup(sqa,sq0,sqb,dx)` 后使用 `i2=nint(90*dx)`；最终 `ldt=false`
    的 `sqf(i2)` 才写回 `dd0`。
- 在当前 `+0.785s` 窗口下，包络和 refined-DT `sqf()` 都对齐后，RustFFT 与
  FFTW 长测都保持 `424/449`，可保留。

### 77-bit Message Family

- 本项目只专注 FT8。JT77 中的 WSPR-style Type 0.6 不作为功能目标。
- 补齐 FT8 相关 receive/pack 分支：
  - Type 0.1 DXpedition `RR73;`
  - Type 0.3/0.4 ARRL Field Day
  - Type 0.5 telemetry
  - Type 3 ARRL RTTY
  - Type 5 EU VHF hashed-call exchange
- 补齐 WSJT-X 接收侧 hard gate：
  - standard callsign `callok()` 风格校验。
  - 全局 `CQ <...>` reject。
  - 非 contest 下拒绝 `i3=1..3` 且消息包含 `/R` 或以 `TU;` 开头。
- 补齐 receive unpack 的 `mycall/hiscall` hash 替换上下文。

## 重点 Miss 诊断

### `230208_140430 F4JAR UX7UU -19`

- baseline SNR `-9`、drift `0.1`、freq `1413`，曾是最强标准消息 miss。
- 目标并非被 `sync8` 漏掉；候选进入 `ft8b`，hard Costas `nsync≈18`。
- 失败点在 bit metrics / LDPC / OSD：当前细化时间附近可恢复正确 message，
  但 hard errors 约 `40..43`，超过 WSJT-X `nharderrors<=36` 接受线。
- 临时时间扫描显示，`ibest` 往后约 `+6/+7` 个 200 Hz 样点时 hard errors 可降
  到 `33..34`，理论上足够通过。
- RustFFT 与 FFTW 结果一致；临时 `fftwf` 探针也选到同一同步峰。该 miss
  不像 FFT 后端、pack/unpack 或 AP pass 未执行。
- 用户后来确认这条是 JTDX decode，当前 WSJT-X 对齐阶段不再优先追它。

### `230208_140415 FO0L F4GYE JN07`

- 是窗口敏感 lost decode：
  - 单 slot/full decode 在 offset `0.000/0.250/1.000` 可解。
  - 在 offset `0.500/0.785` 即使窄频、`ncand=5000`、`syncmin=0.5` 也未解。
- 更像时间窗口/邻近信号相位关系导致，不是 AP memory 或 progressive subtract
  独立造成。

### `230208_140445 VE7ON S56KFG JN76`

- 在 `+0.785s` 窗口是 near-dupe 边界问题。
- 默认 pass2 中不可解强候选 `1446.875Hz / dt=+1.140 / sync≈2.20` 会压掉可解
  候选 `1443.750Hz / dt=+1.180 / sync≈1.94`。
- 两者时间差正好 `0.04s`。WSJT-X 单精度 `tdiff < 0.04` 不应合并该边界；
  Rust f64 原先会因 roundoff 当成略小于 `0.04`。
- 修复边界后该类问题缓解。

## 下一步

优先级不变：

1. 通过 WSJT-X 源码查架构差异。
2. 通过 miss 查架构差异。
3. 通过 WSJT-X 源码查参数差异。
4. 通过 miss 查参数差异。

下一轮重点：

- 继续核对 `subtractft8` 剩余边界：`peakup` 极小分母、Fortran `nint` 半值、
  `x(NFFT+2)`/real-to-complex storage 边界，以及 FFTW 单精度路径对比。
- 查 `ft8_decode` 外层 `dd0/dd1/newdat/subtract` 生命周期，尤其 progressive
  residual 和 duplicate/subtract 的交互。
- 查 `ft8_a7` same-parity memory、`ft8_a7_save` 调用时机、`hashcallbook`
  共享和 AP 表裁剪。
- 对 remaining diff 做 cluster：按 slot、freq、message family、drift 和 tag
  分类，不把 display-only 差异当成真实 miss。

## 最近验证

- `cargo test --release test_stream_decode_short_audio -- --nocapture` ✅
  - `21` unique messages
  - 约 `3.3s`
- `FT8RS_WRITE_DIFF=1 cargo test --release test_stream_decode_long_audio -- --nocapture` ✅
  - `424/449`
  - timing residual median `+0.000s`
  - 每段均小于 `15s`，最慢约 `3.68s`
- `FT8RS_WRITE_DIFF=1 cargo test --release --features fftw test_stream_decode_long_audio -- --nocapture` ✅
  - `424/449`
  - timing residual median `+0.000s`
  - 每段均小于 `15s`，总耗时约 `54.7s`
- `git diff --check` ✅
