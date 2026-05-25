# TRY.md — FT8 流式解码尝试记录

## Milestone 1: WSJT-X 架构对齐到长短验收通过

### 目标
- 短文件 `210703_133430.wav`：至少 `19/20`，单段小于 `15s`。
- 长文件 `230208_140300.wav`：第一里程碑至少 `381/449`，每段小于 `15s`。
- 验收路径必须使用 release + FFTW@3840；RustFFT@4096 只保留为便携/对照路径。

### 合并后的主要尝试
- 重新阅读并持续对照 WSJT-X 关键源码：
  - `wsjtx/lib/jt9a.f90`
  - `wsjtx/lib/decoder.f90`
  - `wsjtx/lib/ft8_decode.f90`
  - `wsjtx/lib/ft8/ft8b.f90`
  - `wsjtx/lib/ft8/sync8.f90`
  - `wsjtx/lib/ft8/sync8d.f90`
  - `wsjtx/lib/ft8/ft8_a7.f90`
  - `wsjtx/lib/ft8/ft8_downsample.f90`
  - `wsjtx/lib/ft8/get_spectrum_baseline.f90`
  - `wsjtx/lib/ft8/baseline.f90`
  - `wsjtx/lib/pctile.f90`
  - `wsjtx/lib/ft8/decode174_91.f90`
  - `wsjtx/lib/ft8/osd174_91.f90`
  - `wsjtx/lib/77bit/packjt77.f90`
- 将 stream decoder 从“full decode + AP”改成 WSJT-X 风格 `nzhsym=41/47/50` 渐进流：
  - `41*3456` 后清零并 early decode
  - `47*3456` 后清零，对 early decodes 做 subtract，保存 `dd1`
  - full pass 使用 cleaned prefix + 原始 remainder，并继续 subtract 未清理 early decodes
- 修正 `sync8` 结构：
  - FFTW@3840 作为默认对齐路径
  - `mlag=13`
  - `MAXCAND=1000`
  - descending `red`、可选 `red2`
  - near-dupe pruning
  - `nfqso +/- 10 Hz` priority
  - `sbase` 来自 `get_spectrum_baseline(dd,nfa,nfb)`
- 修正 `sbase/baseline` 坐标和数值语义：
  - 保留 Vec index 0，对齐 Fortran 1-based `sbase(1:NH1)`
  - 跳过 DC bin 0
  - `pctile` 使用 `nint(npts*0.01*npct)`
  - lower-envelope 点集按 WSJT-X 1000 点上限处理
- 修正 `ft8_decode/ft8b` 外层/内层 decode 架构：
  - depth 1 两 pass，depth 2/3 三 pass
  - pass 1 `imetric=1`，pass 2/3 `imetric=2`
  - `imetric=2` 时 `s2=s2**2`
  - regular pass 变为 `llra/llrb/llrc/llrd/llre`
  - hard sync gate 对齐 `nsync <= 6/7/8`
  - `nagain` 接入 `nfqso +/- 20 Hz` 和 SNR 选择
  - valid duplicate decode 也先 subtract，再 duplicate skip
  - 每个 `sync8` pass 都刷新当前 residual 的 `sbase`
- 修正 AP 和跨时隙记忆：
  - `StreamDecodeConfig` 暴露 WSJT-X AP/QSO 参数
  - 内部 AP pass 结构接近 `nappasses/naptypes/nQSOProgress`
  - `ft8apset`、contest/Hound AP mask 分支初步对齐
  - `ft8_a7_save` 的 `split77`/`CQ_` 保存语义部分对齐
  - same-parity previous/current AP entry 抑制对齐 `f0=-98`
  - stream `ft8_a7d` 使用 full regular decode 后 residual
  - `ft8_a7d` 成功结果也保存到 same-parity AP memory
- 修正 LDPC/OSD 控制流：
  - `maxosd=0` 使用 channel LLR
  - `maxosd>0` 使用 BP posterior `zsum`
  - `maxosd` capped at 3
  - 删除非 WSJT-X raw LLR fallback
  - BP 成功时计算 `dmin`
  - OSD 只在 `nharderrors > 0` 时接受
  - `osd174_91` 主路径改成 WSJT-X `ndeep=2` 第一预筛规则
- 清理测试门禁：
  - stream acceptance tests 运行时断言 release
  - stream acceptance tests 运行时断言 `engine_name()=="FFTW"`
  - 长测每段 `15s` timeout
  - 长测保留 `target-10` 灵敏度早停
  - `FT8RS_PRINT_MISSES=1` 可打印长测未匹配 baseline 消息
- 关键突破：
  - Rust 之前在 `ft8b` 的 `find_best_time_offset`、`find_best_frequency_shift`、`refine_time_offset` 中对 `sync8d` 时间索引用 `rem_euclid(NP2)` 环绕。
  - WSJT-X `sync8d(cd0,i0,...)` 使用 signed `i0`，越界 Costas block 直接贡献 0，不会环绕到 `cd0` 尾部。
  - 去掉这个环绕后，长测从 `361/449` 提升到 `381/449`，并通过第一里程碑。

### 无效或低收益尝试
- 单纯切换 FFT 表示或保留 RustFFT@4096 不能作为 WSJT-X 对齐依据。
- 调整阈值、放宽长测断言不是有效路线。
- residual/sbase/AP 输入的若干结构修正虽然更接近 WSJT-X，但单独没有提升 `361/449`；保留这些改动是因为它们减少架构偏差。

### 当前测试结果
- `cargo check` ✅
- `cargo check --tests` ✅（只编译 test target）
- `git diff --check` ✅
- `cargo test --release test_stream_decode_short_audio -- --nocapture` ✅
  - `21` unique messages
  - 约 `3.9s`
- `cargo test --release test_stream_decode_long_audio -- --nocapture` ✅
  - `381/449`
  - 每段均小于 `15s`

## Milestone 2: 从 381 推进到目标 400

### 解码要求
- 最低保持：`381/449`
- 第二里程碑目标：`400/449`
- 不允许通过放宽阈值、切换 RustFFT@4096、扩大非 WSJT-X 搜索来“冲分”。

### 推进优先级
1. 通过源代码架构上的差异继续查。
2. 通过 miss 查找架构上的差异。
3. 通过源代码查找参数差异。
4. 通过 miss 查找参数差异。

### 当前高优先级待查
- `ft8_downsample/twkfreq1` 与 WSJT-X 的 FFT shift、边界、归一化是否还有结构差异。
- `ft8b` bit metric 构造、`normalizebmet`、`llr` 送入 `decode174_91` 的数值路径是否完全同构。
- `osd174_91` 仍只完成当前主路径 `ndeep=2` 第一预筛；`npre2`/`boxit91`/`fetchit91` 和更深分支还未完整移植。
- `ft8_a7d` 与 WSJT-X 的 AP brute force、`dmin/dmin2`、`xsnr`、grid/report 处理仍需做源码级再对照。
- `HashCallBook` 的 save/lookup 调用点与 `packjt77.f90` 仍需继续核对。
- `baseline/polyfit` 需要 fixture 对比，确认 Rust normal-equation 实现是否足够接近 WSJT-X。

## Milestone 2 / Iteration 1: AP `sync8d_twk`、pass 内 FFT 生命周期、`sync8` 起点对齐

### 做了什么
- 继续按源码架构差异优先级对照：
  - `wsjtx/lib/ft8/ft8_a7.f90`
  - `wsjtx/lib/ft8/sync8d.f90`
  - `wsjtx/lib/ft8/ft8b.f90`
  - `src/ft8/ap_decode.rs`
  - `src/ft8/decode.rs`
- 修正 AP `ft8_a7d` 里的 sync refine 架构差异：
  - WSJT-X `sync8d(..., itwk=1)` 使用 `ctwk * csync(i)`
  - Rust 之前的 `ap_sync8d_twk` 只用了 `ctwk`，没有乘 Costas waveform
  - 现在 AP frequency refine 使用 `ctwk * Costas`
  - AP second time refine 改回无 frequency tweak 的 Costas sync
- 修正 pass 内 long FFT 生命周期差异：
  - WSJT-X `ft8_decode` 每个 pass 开始设置 `newdat=.true.`
  - 第一次 `ft8_downsample` 后 Fortran 通过引用把 `newdat=.false.`
  - 同一 pass 后续 candidate 即使 valid decode 触发 subtract，也不会重算 long FFT
  - Rust 之前每次 subtract 后立即重算 long FFT，让同一 pass 后续 candidate 看到 cleaned residual
  - 现在 Rust 只更新 residual，long FFT 在下一 pass 开始重算，贴近 WSJT-X
- 修正 `sync8` Costas 搜索起点：
  - WSJT-X `sync8.f90` 的 `jstrt=0.5/tstep` 因隐式类型规则是整数赋值
  - `12.5` 在 Fortran 中截断为 `12`
  - Rust 之前用 `round()` 得到 `13`
  - 现在改为截断，避免用非 WSJT-X 起点获得偶然候选
- 修正 AP SNR 参数差异：
  - WSJT-X `ft8_a7d` 使用 `xsnr=-25` 作为初值和下限
  - Rust 之前是 `-24`
  - 现在对齐为 `-25 dB`

### 测试结果
- `cargo check` ✅
- `cargo check --tests` ✅（只编译 test target）
- `git diff --check` ✅
- `cargo test --release test_stream_decode_short_audio -- --nocapture` ✅
  - `21` unique messages
  - 约 `5.4s`
- `cargo test --release test_stream_decode_long_audio -- --nocapture` ✅
  - `381/449`
  - 每段均小于 `15s`

### 反思
- AP sync refine 曾带来 `381 -> 382` 的小幅提升，但继续对齐 `sync8`
  的 Fortran 整数截断后回到 `381/449`。
- 这说明 `382` 里至少有 1 条来自非 WSJT-X 的 `jstrt` rounding 偏差；
  第二里程碑仍应以源码一致性优先，而不是保留偶然增益。
- pass 内 long FFT 生命周期对齐没有降低结果，保留该结构修正。
- 当前仍离第二里程碑 `400/449` 差 `19` 条，继续按源码架构差异优先推进。

## Milestone 2 / Miss Review 1: 缺失消息反推

### 做了什么
- 按优先级 2 使用 `FT8RS_PRINT_MISSES=1` 跑长测，只观察 miss 分布，不调阈值。
- 结果仍为 `381/449`，每段均小于 `15s`。

### 观察
- 多数 miss 仍集中在 `-17 dB` 到 `-23 dB` 的弱信号，例如重复出现的
  `CQ D1DX KN87`、`CQ F1PPH JN07`、`CQ R6KEE KN75`。
- 有一类不是纯灵敏度问题：compound/nonstandard call 消息，例如
  `EA5/DH0YAH RK4FF RR73`、`RK4FF EA5/DH0YAH 73`。
- 当前 decoder 实际能解出同一类 Type 4 消息，但形式是
  `EA5/DH0YAH <RK4FF> RR73` / `<RK4FF> EA5/DH0YAH 73`。

### 反思
- Type 4 的尖括号显示与 WSJT-X `packjt77.f90` 的 `hash12` 返回形式一致；
  不应为了匹配 CSV 直接在核心 decoder 里去掉尖括号。
- 这类消息说明后续要继续核对 `HashCallBook` 的 save/lookup 调用点、recent
  calls 语义和测试匹配规范，但不能先用 display normalization 冒充解码提升。
- 弱 CQ 重复 miss 更可能仍在 `sync8` candidate、bit metric/OSD 或 subtraction
  residual 细节里，需要继续回到源码差异。
