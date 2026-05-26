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
  - `wsjtx/lib/77bit/packjt77.f90`
- 将 stream decoder 从“full decode + AP”改成 WSJT-X 风格 `nzhsym=41/47/50` 渐进流。
- 修正 `sync8`、`sbase/baseline`、`ft8_decode/ft8b` 外层 pass、regular bit metrics、hard sync gate、subtract/residual、AP memory、LDPC/OSD 主路径。
- 建立 release + FFTW@3840 测试门禁：
  - stream acceptance tests 运行时断言 release。
  - stream acceptance tests 运行时断言 `engine_name()=="FFTW"`。
  - 长测每段 `15s` timeout。
  - 长测保留 `target-10` 灵敏度早停。
  - `FT8RS_PRINT_MISSES=1` 可打印长测未匹配 baseline 消息。

### 关键突破
- Rust 之前在 `ft8b` 的 `find_best_time_offset`、`find_best_frequency_shift`、`refine_time_offset` 中对 `sync8d` 时间索引用 `rem_euclid(NP2)` 环绕。
- WSJT-X `sync8d(cd0,i0,...)` 使用 signed `i0`，越界 Costas block 直接贡献 0，不会环绕到 `cd0` 尾部。
- 去掉这个环绕后，长测从 `361/449` 提升到 `381/449`，并通过第一里程碑。

### 无效或低收益尝试
- 单纯切换 FFT 表示或保留 RustFFT@4096 不能作为 WSJT-X 对齐依据。
- 调整阈值、放宽长测断言不是有效路线。
- residual/sbase/AP 输入的若干结构修正虽然更接近 WSJT-X，但单独没有提升 `361/449`；保留这些改动是因为它们减少架构偏差。

### 验证
- `cargo check` ✅
- `cargo check --tests` ✅
- `git diff --check` ✅
- `cargo test --release test_stream_decode_short_audio -- --nocapture` ✅
  - `21` unique messages
- `cargo test --release test_stream_decode_long_audio -- --nocapture` ✅
  - `381/449`
  - 每段均小于 `15s`

## Milestone 2: 从 381 推进到 401，达成目标 400

### 目标
- 最低保持：`381/449`
- 第二里程碑目标：`400/449`
- 不允许通过放宽阈值、切换 RustFFT@4096、扩大非 WSJT-X 搜索来“冲分”。

### 有效源码对齐
- AP `sync8d` 频率微调用 `ctwk * Costas`，second time refine 回到普通 Costas sync。
- `ft8_decode` pass 内 FFT 生命周期对齐 WSJT-X：同一 pass 内 subtract 后不立刻重算 long FFT，下一 pass 再刷新。
- outer `syncmin` 默认值对齐：depth 1/2 使用 `2.1`，depth 3 使用 `1.3`。
- `sync8` time-bin 明确按 Fortran 1-based 时间维访问，Rust 访问前转换为 `m-1`。
- stream `ft8_a7_save` 模拟层按 WSJT-X `split77/chkcall` 语义处理 CQ：
  - `CQ D1DX KN87` 不应被误写成 `CQ_D1DX KN87`。
  - `CQ DX DL8YHR JO41` 仍应写成 `CQ_DX DL8YHR JO41`。
- `ft8_a7d` 把 compact Rust `call_1=="CQ"` 视作 Fortran padded `call_1(1:3)=="CQ "`。
- `pack_jt77::is_stdcall()` 修正 Fortran 1-based `iarea` 到 Rust 0-based 的转换：
  - WSJT-X `iarea in 2..3` -> Rust `iarea in 1..2`
  - WSJT-X `npdig < iarea-1` -> Rust `npdig < iarea`

### 关键突破
- 弱 CQ 反复 miss 的主因之一是 `stdcall` 1-based/0-based 移植错误。
- 旧 Rust 把 `D1DX`、`F1PPH`、`R6KEE`、`IW1PUR` 误判为非标准呼号。
- `ft8_a7d` CQ `imsg=5` 因此生成 `CQ CALL`，没有生成 WSJT-X 的 `CQ CALL GRID`。
- 修正后长测从 `384/449` 提升到 `401/449`，达到第二里程碑目标。

### 诊断工具
- 默认关闭的 `FT8RS_DUMP_SYNC8`：
  - 输出 `[SYNC8_PRE]` 和 `[SYNC8_FINAL]`，用于和 WSJT-X `sync8.f90` 数值 fixture 对比。
- 默认关闭的 `FT8RS_TRACE_TARGETS`：
  - 输出 `[TRACE_SYNC8]`、`[TRACE_FT8B]`、`[TRACE_DECODE]`、`[TRACE_AP_*]`，用于单条 miss 跟踪。
- `FT8RS_PRINT_MISSES=1`：
  - 输出长测剩余 miss，避免凭感觉选排查方向。

### 无效或撤回尝试
- `sync8` 修正后临时把 candidate dt 改成 `(jpeak-1.5)*tstep` 可以回到 `381/449`，但这不是 WSJT-X 公式，已撤回。
- CQ `call_1` padding 修复是正确源码语义，但单独没有提升分数；保留作为防回归。
- 不能通过放宽 `dmin/dmin2`、hard sync gate、syncmin 或候选搜索范围追分。

### 验证
- `cargo test --release stdcall_matches_wsjtx_one_based_iarea -- --nocapture` ✅
- `cargo test --release ap_message -- --nocapture` ✅
- `cargo test --release split77_words -- --nocapture` ✅
- `cargo test --release test_stream_decode_short_audio -- --nocapture` ✅
  - `21` unique messages
  - 约 `4.4s`
- `cargo test --release test_stream_decode_long_audio -- --nocapture` ✅
  - `401/449`
  - 每段均小于 `15s`
- `FT8RS_PRINT_MISSES=1 cargo test --release test_stream_decode_long_audio -- --nocapture` ✅
  - `401/449`
  - 剩余少量弱 CQ：`CQ F1PPH JN07`、`CQ D1DX KN87`、`CQ R6KEE KN75`、`CQ IW1PUR JN44`、`CQ OH5NBJ KP41`、`CQ IZ7MFY JN81`
  - 非 CQ 缺口也开始变多：compound/hash、报告、重叠强信号附近的常规消息
- `cargo check --tests` ✅
- `git diff --check` ✅

### 第三里程碑线索
- 目标：`420/449`。
- 先全面排查 1-based/0-based 移植点，尤其是：
  - Fortran 数组下标进入 Rust Vec/array 的边界。
  - Fortran `nint`、隐式整数赋值、`maxloc/minloc` 返回 1-based index 的地方。
  - `packjt77/unpack77` 里呼号、grid、report 的编码下标。
  - `sync8/sync8d/ft8b/ft8_a7d` 时间、频率、symbol 索引链路。
- 然后继续按优先级推进：
  1. 通过源代码架构上的差异查。
  2. 通过 miss 查找架构上的差异。
  3. 通过源代码查找参数差异。
  4. 通过 miss 查找参数差异。

## Milestone 3: 目标 420，第一轮 1-based/0-based 审计

### 本轮目标
- 在第二里程碑 `401/449` 基线上，先清理记录、保住成果，再继续从源码层面排查容易忽略的 Fortran 1-based/Rust 0-based 差异。
- 不为了涨分改阈值；所有改动必须能指向 WSJT-X 源码语义。

### 已处理的源码差异
- `pack77_1` 的 `R GRID` 判定：
  - WSJT-X 检查第三个词 `w(3) == 'R '`。
  - Rust 之前误看第二个词，现改为 `parts[2] == "R"`。
- `split77/chkcall` 的标准呼号判定：
  - WSJT-X `split77` 调 `chkcall(w(3))`，`RR73`、`73` 这类报告词不能被当作标准呼号。
  - Rust `parse_callsign` 之前只限制 suffix 字母数 `<=3`，会把 `RR73` 误判为标准呼号；现按 `chkcall` 收紧为必须有 `1..=3` 个字母 suffix，并补齐首两位字母和 `Q` 前缀规则。
  - 注意：AP 路径的 `is_stdcall()` 仍保留 WSJT-X `q65_set_list.f90:stdcall` 风格，因为它本来就比 `chkcall` 松，不能混用。
- `unpack77` 的 CQ invalid guard：
  - WSJT-X 对 `CQ ... R GRID` 判 invalid。
  - WSJT-X 对 `CQ ... RRR/73/report` 这类 `irpt>=2` 判 invalid。
  - Rust 现补齐对应拒绝逻辑。
- `sync8` 的线程局部 `savg` 每次进入前清零，保持 Fortran `savg=0.` 的生命周期语义；当前 `sbase` 已走独立 Welch 路径，这个改动主要是清理潜在状态残留。
- stream AP memory 的 `chkcall` 镜像：
  - WSJT-X `chkcall.f90` 先检查第 2 位数字、再检查第 3 位数字；如果两者都是数字，第 3 位会覆盖为 call area。
  - Rust 之前用 `if/else if`，会提前选第 2 位；现改成和 Fortran 赋值顺序一致。
- `pack77_1` 两词消息 guard：
  - WSJT-X `nwords==2` 且第二词包含 `/` 时直接 return。
  - Rust 现拒绝 `CALL1 CALL2/R` 这类两词 Type 1 形态，避免 AP brute-force 产生 WSJT-X 不会产生的候选。

### 需要记住的细节
- `RR73` 同时满足 4 字符 grid 形态 `RR73`，WSJT-X `pack77_1` 后续会先按 grid 分支处理；因此不能用 `CQ K1ABC RR73` 来测试 `irpt=3` invalid guard，测试应使用 `RRR` 或 `73`。

### 验证
- `cargo fmt` ✅
- `cargo check --tests` ✅
- `git diff --check` ✅
- `cargo test --release util::pack_jt77::tests -- --nocapture` ✅
- `cargo test --release util::unpack_jt77::tests -- --nocapture` ✅
- `cargo test --release stream::decoder::tests -- --nocapture` ✅
- `cargo test --release test_stream_decode_short_audio -- --nocapture` ✅
  - `21` unique messages
  - 约 `4.4s`
- `cargo test --release test_stream_decode_long_audio -- --nocapture` ✅
  - `401/449`
  - 每段均小于 `15s`

### 下一步
- 继续 1-based/0-based 审计：
  - `ft8_downsample` 的 `cshift/i0/ib` 边界。
  - `ft8b` 的 `maxloc(ss)`、`ibest`、`xdt=(ibest-1)*dt2` 链路。
  - `ft8_a7d` AP brute-force 的 `imsg`、`s8(0:7,1:NN)`、`dmm(1:206)` 下标。
- 若源码审计没有明显缺口，再用剩余 miss 做单条 trace 对比。
