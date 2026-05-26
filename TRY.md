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

## Streaming: 声卡实时 `nzhsym=41/47/50` 分阶段调度

### 目标
- 不改变解码器数学逻辑和灵敏度，只把原来“收满 15 秒后顺序跑 41/47/50”的适配层改成实时触发。
- 对齐 WSJT-X 的流式节奏：
  - `nzhsym=41`：采到 `41*3456/12000 = 11.808s` 后先解强信号。
  - `nzhsym=47`：采到 `13.536s` 后做 early subtract。
  - `nzhsym=50`：采到完整 slot 后做 full decode + AP。

### 改动
- `StreamDecodeSession` 拆出可显式调用的 stage API：
  - `start_slot_decode`
  - `decode_slot_nzhsym41`
  - `subtract_slot_nzhsym47`
  - `decode_slot_nzhsym50_and_finish`
- 原 `decode_slot_streaming` 改为调用同一套 stage API，保证文件/测试路径和声卡路径共用解码流程。
- 声卡 `decode_soundcard_streaming_decodes` 改为实时 staged collector：
  - 采到 41 threshold 后立即运行 early decode 并输出。
  - 采到 47 threshold 后执行 subtract。
  - 收满 15 秒后执行 full/AP，并输出 slot done。
- 声卡采集增加 `NativeSampleCollector` carry buffer：
  - 原实现如果一个 audio chunk 超出 slot 末尾，会截断并丢掉尾部。
  - 现在把超出的 native samples 留给下一个 slot，避免长期运行时 slot 边界漂移。

### 验证
- `cargo fmt` ✅
- `cargo check --tests` ✅
- `cargo test --release test_stream_decode_short_audio -- --nocapture` ✅
  - `21` unique messages
  - 约 `4.4s`
- `cargo test --release test_stream_decode_long_audio -- --nocapture` ✅
  - `422/449`
  - 总耗时约 `83.88s`
  - 每段均小于 `15s`
  - timing residual median 仍为 `+0.785s`
- `cargo build --release` ✅
- `target/release/ft8rs soundcard --device "VB-Cable A" --slots 2` ✅
  - 实测两段正常输出，slot done 分别为 `9` 和 `6` decodes。

## Performance: WSJT-X `newdat`/`save cx` 对齐

### 对照结论
- WSJT-X `ft8_downsample.f90` 使用 `save x,cx` 保存 192000 点长 FFT。
- `ft8_decode.f90` 在 AP 循环前设置一次 `newdat=.true.`；第一次 `ft8_a7d` 调用刷新长 FFT，后续 AP 候选复用同一个 `cx`。
- `ft8_a7d` 内部第二次 refined downsample 传 `.false.`，同样复用该 `cx`，只重新抽取频带、taper、cshift 和 3200 点 IFFT。

### 改动
- 新增 `ApDownsampleCache`，显式保存一个 slot residual 的长 FFT。
- `StreamDecodeSession` 在 AP 候选循环前创建一次 cache，所有 `ft8_a7d` 候选共享。
- `ft8_a7d` 保留独立入口；单独调用时会创建自己的 cache，行为兼容。
- `ap_downsample` 改为从 cache 中抽带，保持 WSJT-X `ft8_downsample` 的 `ib/it/i0/taper/cshift/IFFT/fac` 链路不变。
- `gen_ft8wave` 增加 65536 点 complex phase table cache，对齐 WSJT-X 的 `ctab(0:NTAB-1)` 缓存方式。

### 说明
- 这轮没有减少候选数、没有修改门限、没有关闭 AP、没有改变 `nzhsym=41/47/50` 流程。
- AP cache 是明确收益点：Rust 原来每个 AP 候选做两次 192000 点 FFT；WSJT-X 是每个 AP stage 做一次长 FFT。
- `ctab` 查表主要是 WSJT-X 对齐和避免重复三角函数；在当前长测里收益不明显，瓶颈更可能仍在 subtract 的 LPF FFT 和候选 LDPC/OSD。

### 验证
- `cargo fmt` ✅
- `cargo check --tests` ✅
- `cargo test --release test_stream_decode_short_audio -- --nocapture` ✅
  - `21` unique messages
  - 约 `4.5s`
- `cargo test --release test_stream_decode_long_audio -- --nocapture` ✅
  - `422/449`
  - 总耗时约 `85.13s`
  - 每段均小于 `15s`
  - timing residual median 仍为 `+0.785s`

## Performance: 第一轮不降灵敏度优化

### 目标
- 只减少 Rust 实现中的重复计算和重复分配。
- 不降低 `ncand`、`ndepth`，不关闭 AP，不改 sync gate，不改变 WSJT-X 对齐参数。

### 有效改动
- 删除 `decode_from_f64` 进入 pass loop 前的一次未使用 FFT。
  - 该 FFT 后续每个 pass 都会重新计算，结果没有被读取。
- `decode_from_f64` 的候选解码工作区从“每个 candidate 新建一次”改为“每个 pass 复用一次”。
  - 对齐 WSJT-X 固定数组反复覆盖的风格。
  - `ft8b` 会重写 `cd0/s8/cs/metrics/llr/apmask` 等候选局部状态。
- 去掉 `ft8b` 内重复的 hard sync 统计。
  - 原来先 `compute_nsync()`，再通过 `passes_sync_gate_strict()` 再算一遍。
  - 现在直接用同一个 `nsync` 做 gate 和低 SNR false-positive gate。
- 避免同一个 codeword 的 tone 序列重复生成。
  - `compute_snr()` 和 `itone` 输出共用同一份 `tones`。

### 无效尝试
- 尝试把 pass 内 coarse downsample cache 激活。
  - 结果长测从约 `88s` 变慢到约 `107s`。
  - 原因是缓存整段 `NFFT2` 复数数组需要大块 clone，收益抵不过内存拷贝。
  - 已撤回。
- 尝试把 `sync8` 的 `red/red2/jpeak/order` 等小数组放进 thread-local buffer。
  - 短测稳定变慢到约 `5.8s`。
  - 推测局部新建小 `Vec` 更利于当前编译器优化和 cache locality。
  - 已撤回。

### 验证
- `cargo fmt` ✅
- `cargo check --tests` ✅
- `cargo test --release test_stream_decode_short_audio -- --nocapture` ✅
  - `21` unique messages
  - 约 `4.4s`
- `cargo test --release test_stream_decode_long_audio -- --nocapture` ✅
  - `422/449`
  - 总耗时约 `86.05s`
  - 每段均小于 `15s`
  - timing residual median 仍为 `+0.785s`
- `git diff --check` ✅
- `cargo run --release -- --fft-engine fftw file tests/ft8/210703_133430.wav` ✅
  - CLI 短文件仍输出 `21` 条。
- `target/release/ft8rs --fft-engine fftw file tests/ft8/230208_140300.wav --low 200 --high 210 --depth 1` ✅
  - 长文件快速 smoke test 仍输出 19 个 slot 的段间分隔符。
- `cargo test --release test_stream_decode_short_audio -- --nocapture` ✅
  - `21` unique messages
  - 约 `4.6s`
- `cargo test --release test_stream_decode_long_audio -- --nocapture` ✅
  - `422/449`
  - 每段均小于 `15s`
  - timing residual median 仍为 `+0.785s`

## Engineering: util 收口与时间职责拆分

### 目的
- 让解码器保持相对独立，上层不要直接依赖内部 `util` 模块。
- `stream::time` 只表达 slot 时间，不承担文件名解析职责。
- 单一 owner 的 util 代码合并回 owner，减少“公共工具箱”膨胀。

### 改动
- `stream::time::SlotTimestamp` 只保留：
  - `parse`
  - `add_seconds`
  - `format` / `Display`
- 文件名时间戳推断移动到 `input::file::infer_start_time_from_path`。
- `util` 从公开模块改为 crate-internal：
  - 上层仍可通过 root 使用 `HashCallBook`。
  - 测试和上层可通过 root 使用 `fft_engine_name`，不再访问 `ft8rs::util`。
- 合并单一 owner util：
  - `util::crc` 合入 `ft8::decode174_91`。
  - `util::ldpc` 合入 `ft8::ap_decode`，只服务 AP brute-force codeword 生成。
- 删除未使用 FFT API：
  - `fft_c2r`
  - `next_pow2`
  - 对应 FFTW c2r plan/cache/FFI 和 RustFFT wrapper。
- 将只服务 FT8/JT77 解码器的协议模块从 `util` 移到 `src/ft8`：
  - `constants` -> `ft8::protocol`
  - `pack_jt77`
  - `unpack_jt77`
  - `decode174_91`
  - `ldpc_tables`
  - `hashcall`
  - `subtract_ft8`
- `util` 现在只保留 FFT dispatcher/engine 这种真正跨层基础设施。

### 验证
- `cargo fmt` ✅
- `cargo check --tests` ✅
  - 无 warning
- `git diff --check` ✅
- `cargo test --release ft8::pack_jt77::tests -- --nocapture` ✅
- `cargo test --release ft8::unpack_jt77::tests -- --nocapture` ✅
- `target/release/ft8rs --fft-engine fftw file tests/ft8/210703_133430.wav` ✅
  - CLI 短文件仍输出 `21` 条。
- `cargo test --release test_stream_decode_short_audio -- --nocapture` ✅
  - `21` unique messages
  - 约 `4.2s`
- `cargo test --release test_stream_decode_long_audio -- --nocapture` ✅
  - `422/449`
  - 每段均小于 `15s`
  - timing residual median 仍为 `+0.785s`

### 下一步
- 继续 1-based/0-based 审计：
  - `ft8_downsample` 的 `cshift/i0/ib` 边界。
  - `ft8b` 的 `maxloc(ss)`、`ibest`、`xdt=(ibest-1)*dt2` 链路。
  - `ft8_a7d` AP brute-force 的 `imsg`、`s8(0:7,1:NN)`、`dmm(1:206)` 下标。
- 若源码审计没有明显缺口，再用剩余 miss 做单条 trace 对比。

## Milestone 3: subtractft8 1-based sample index 修复

### 问题
- WSJT-X `subtractft8.f90` 内部 `nstart` 和 `j` 都是 Fortran 1-based sample index。
- Fortran 循环 `do i=1,nframe; j=nstart-1+i` 的第一轮访问 `dd(nstart)`。
- Rust 之前写成 `for i in 0..NFRAME { j=nstart-1+i; dd0[j-1] }`，第一轮实际访问 `dd0[nstart-2]`，比 WSJT-X 早 `1` 个 sample。
- 同样的偏移存在于主 subtract 和 `lrefinedt` 的 residual energy 评估，导致 refined offset 选择和最终 residual 写回都偏 1 sample。

### 修复
- 增加 `wsjtx_subtract_sample_index(nstart_1based, rust_i)`，明确 Rust `i=0` 对应 Fortran `i=1`，因此 `j=nstart+rust_i`。
- 四处统一改为该映射：
  - `subtract_ft8_refined()` 的 IQ mix。
  - `subtract_ft8_refined()` 的 subtract 写回。
  - `compute_residual_energy()` 的 IQ mix。
  - `compute_residual_energy()` 的 residual energy 计算。
- 添加单测锁住 `nstart` 映射，避免后续再次误减。

### 验证
- `cargo fmt` ✅
- `cargo test --release subtract_sample_index_matches_wsjtx_one_based_loop -- --nocapture` ✅
- `cargo check --tests` ✅
- `git diff --check` ✅
- `cargo test --release test_stream_decode_short_audio -- --nocapture` ✅
  - `21` unique messages
  - 约 `4.4s`
- `cargo test --release test_stream_decode_long_audio -- --nocapture` ✅
  - `402/449`
  - 每段均小于 `15s`

### 结果
- 长测从第二里程碑稳定基线 `401/449` 提升到 `402/449`。
- 这是 residual/subtract 链路的源码对齐收益，不是阈值调整。

## Milestone 3 收口: diff 工具与 EOF 尾段 flush

### 问题
- `tests/ft8/230208_140300_diff.csv` 表头是 `Date-Time,SNR,Drift,Freq,Msg,Tag`，但所有数据行只有 5 列。
- miss 行把 `-` 直接拼在 `Msg` 末尾，例如 `OZ1DYI SV1SSL 73-`，缺少 `Msg,Tag` 之间的逗号。
- extra 行有 `,+`，但缺少 `Drift` 列，例如 `230208_140330,-4,1205,...,+`。
- 初版 miss-only diff 有 19 条集中在 `230208_140730`，原因不是解码器全部漏掉，而是长测试用
  `floor(duration/15s)` 只解完整 slot。`230208_140300.wav` 总长约 `284.47s`，最后一个
  `230208_140730` slot 还有约 `14.47s` 音频，足够作为文件流尾段 flush 一次。
- 复核之前提到的顺手小问题：
  - `apmag`、`npasses`、SNR `-25 dB` floor 当前与 `wsjtx/lib/ft8/ft8b.f90` 主路径一致。
  - `1.01 / 4+nappasses / -24 dB` 来自其他变体路径，不作为当前 FT8 主解码路径的修复依据。

### 修复
- 修正已提交的 `230208_140300_diff.csv`，所有数据行统一为 6 列。
- 重构 long stream test 的 baseline 解析，保留 `Date-Time/SNR/Drift/Freq/Msg` 字段，而不只保存 normalized message。
- 增加统一的 diff CSV writer：
  - miss 行使用 baseline 原始字段，tag=`-`。
  - 为了便于直接查看当前缺口，diff 文件只写 miss，不混入 extra decode。
  - 默认不写文件；设置 `FT8RS_WRITE_DIFF=1` 时生成 `tests/ft8/230208_140300_diff.csv`。
- 长测试分段数改为 `ceil(duration/15s)`，让文件流结束时的非空尾段也进入 `StreamDecodeSession::decode_slot`。
- 长测通过线从旧的 `366/449` 提高到当前已达成的 `420/449`，用于保住第三里程碑成果。
- 删除尾段 flush 后不再需要的“音频窗口外 baseline”补偿逻辑。

### 验证
- `cargo fmt` ✅
- `cargo check --tests` ✅
- `awk -F, 'NR==1{next} NF!=6{print}' tests/ft8/230208_140300_diff.csv` ✅ 无输出
- `FT8RS_WRITE_DIFF=1 cargo test --release test_stream_decode_long_audio -- --nocapture` ✅
  - 生成 `tests/ft8/230208_140300_diff.csv`
  - `420/449`
  - `230208_140730` 真实参与解码，`19` 条中匹配 `18` 条
  - diff 剩余 `29` 条 miss，全部 `Tag=-`
  - 每段均小于 `15s`

## Milestone 4: 目标 430

### 目标
- 当前保护线：`422/449`。
- 第四里程碑目标：`430/449`。
- 继续坚持 WSJT-X 对齐优先，不通过放宽阈值或扩大非 WSJT-X 搜索追分。

### 排查顺序
1. 通过源代码继续查架构差异，优先 `ft8_decode` 外层控制流、`ft8b` AP pass、`ft8_a7` 跨时隙记忆。
2. 用最新 `27` 条 miss 反查架构差异。
3. 源码层面确认参数差异。
4. 最后才用 miss 做参数差异定位。

### 当前线索
- 最新 diff 已经去掉尾段假 miss，后续还需要排除 hash/display 形态造成的假 `-/+`。
- `230208_140730` 只剩 `<...> IK4LZH JN54` 一条未匹配；这个片段本身不再是切窗问题。

## Milestone 4: diff 加回 extra 与录音起点偏移估计

### 问题
- 只看 miss-only diff 不够定位，extra decode 也能暴露 hash/display、时间窗和 baseline 口径差异。
- 长测 `230208_140300.csv` 的 `Drift` 与 Rust 解码输出 `dt` 存在稳定差值，怀疑 WAV 文件起点不是精确 `230208_140300.000`。

### 修复
- diff CSV 恢复 `Tag=+`：
  - `Tag=-` 为 baseline 有但当前未匹配。
  - `Tag=+` 为当前解码有但同 segment baseline 未消耗。
  - 匹配逻辑改为一条 baseline 消耗一条 decoded result，避免重复消息被错误多次匹配。
- 长测统计所有匹配消息的 `baseline_drift - decoded_dt`，输出 mean/median/p10/p90。
- 增加默认关闭的 `FT8RS_SLOT_START_OFFSET_SEC` 诊断参数，用于按推测的绝对 slot 起点平移文件窗口；非零时只做诊断，不触发正式验收断言。
- CLI 文件入口也从整除 slot 数改为 `div_ceil`，避免实际文件流解码跳过 EOF 尾段。

### 结果
- `FT8RS_WRITE_DIFF=1 cargo test --release test_stream_decode_long_audio -- --nocapture`：
  - `420/449`
  - diff 为 `29` 条 `-`，`13` 条 `+`
  - `baseline_drift - decoded_dt`：
    - mean `+0.760s`
    - median `+0.785s`
    - p10 `+0.745s`
    - p90 `+0.825s`
    - n `420`
- 这说明差值非常集中，更像固定录音起点偏移，而不是随机 drift 误差。按当前符号约定，WAV sample 0 更接近 `230208_140300.785`。
- `FT8RS_SLOT_START_OFFSET_SEC=0.785 cargo test --release test_stream_decode_long_audio -- --nocapture`：
  - `416/449`
  - offset residual median 变成 `+0.000s`
  - 这确认时间偏移估计方向是对的，但直接按 CSV 绝对 slot 边界重切窗反而少 4 条；后续要继续对比 WSJT-X 文件窗口、padding 和 AP memory，而不能简单把 offset 当成追分开关。

## Milestone 4: diff 匹配展示归一化

### 问题
- diff 中出现字符串展示差异造成的假 `-/+`：
  - `EA5/DH0YAH RK4FF RR73` vs `EA5/DH0YAH <RK4FF> RR73`
  - `RK4FF EA5/DH0YAH 73` vs `<RK4FF> EA5/DH0YAH 73`
- 这些消息实际内容一致，尖括号只是 hash/display 形态，不应该作为 miss/extra。

### 修复
- 测试匹配归一化时把 `<CALL>` 归一为 `CALL`。
- `<...>` 保持原样，因为它表示未知 hash，不能安全等同于具体呼号。

### 结果
- `FT8RS_WRITE_DIFF=1 cargo test --release test_stream_decode_long_audio -- --nocapture`：
  - `422/449`
  - diff 为 `27` 条 `-`，`11` 条 `+`
  - `baseline_drift - decoded_dt` 仍然稳定：median `+0.785s`
- 长测保护线从 `420/449` 提升到 `422/449`。

## Milestone 4: offset sweep 临时实验

### 目的
- 验证 `+0.785s` 录音起点偏移是否应该直接作为正式切窗 offset。
- 对比多个 offset 下的匹配数、extra/miss、弱信号 extra、晚到大 drift miss 和 timing residual。

### 临时测试
- 临时新增 `tests/tmp_offset_sweep.rs`，跑完后删除，不保留在仓库。
- 测试使用 release + FFTW。
- 三个完整解码产物永久保留：
  - `tests/ft8/230208_140300_decode_no_offset.csv`
  - `tests/ft8/230208_140300_decode_offset_0785.csv`
  - `tests/ft8/230208_140300_decode_offset_diff.csv`

### 结果

| offset | decoded | matched | misses | extras | weak extras | late drift misses | residual median | p10 | p90 |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 0.000 | 433 | 422 | 27 | 11 | 4 | 6 | +0.785 | +0.745 | +0.825 |
| 0.250 | 435 | 424 | 25 | 11 | 4 | 6 | +0.535 | +0.490 | +0.575 |
| 0.500 | 437 | 426 | 23 | 11 | 4 | 6 | +0.285 | +0.240 | +0.325 |
| 0.785 | 425 | 418 | 31 | 7 | 2 | 10 | +0.000 | -0.045 | +0.040 |
| 1.000 | 420 | 412 | 37 | 8 | 3 | 13 | -0.215 | -0.255 | -0.175 |

### 结论
- `0.785s` 偏移估计是真实的：它能把 `baseline_drift - decoded_dt` residual 拉到 0 附近。
- 但最佳匹配数出现在 `0.500s`，达到 `426/449`，不是 residual 归零的 `0.785s`。
- offset 越接近/超过 `0.785s`，大 drift late misses 明显变多：
  - `0.500s` late drift misses = `6`
  - `0.785s` late drift misses = `10`
  - `1.000s` late drift misses = `13`
- 这支持之前判断：正式方向不应是简单把 slot 窗口硬平移到 `0.785s`，而要继续对齐 WSJT-X 文件窗口、padding、长文件连续缓冲和跨时隙 AP memory。

## Milestone 4: duplicate-gated subtract 假设核对与撤回

### 问题
- 只看 `wsjtx/lib/ft8_decode.f90` 外层 duplicate check，容易误以为 regular subtract 应该在 `.not.ldupe` 之后执行。
- 临时把 Rust subtract 移到 duplicate check 之后，短测仍为 `21`，但长测从保护线 `422/449` 降到 `421/449`。

### 核对结论
- 继续打开 `wsjtx/lib/ft8/ft8b.f90` 后确认，WSJT-X regular subtract 实际在 `ft8b` 内部完成：
  - valid codeword/message 后设置 `nbadcrc=0`
  - `call get_ft8_tones_from_77bits(...)`
  - `if(lsubtract) call subtractft8(dd0,itone,f1,xdt,.false.)`
  - 然后才返回 `ft8_decode.f90` 做外层 duplicate check
- 因此 Rust “ft8b 返回成功后立刻 subtract，再做 `seen_messages` duplicate filter” 才是有效控制流对齐。

### 处理
- 撤回 duplicate-gated subtract 临时代码。
- 保留文档结论：regular decode 的 subtract 位置要按 `ft8b.f90` 的有效执行顺序判断，不能只看 `ft8_decode.f90` 外层 duplicate check。

## Milestone 4: baseline polynomial 阶数对齐

### 问题
- 继续对照 `wsjtx/lib/ft8/baseline.f90` 时发现，WSJT-X 的 `nterms=5` 表示 5 个系数 `a(1:5)`：
  - `a1 + t*(a2 + t*(a3 + t*(a4 + t*a5)))`
  - 这是 4 次多项式。
- Rust `polyfit(&env_x, &env_y, 5)` 的参数语义是 degree，实际生成 6 个系数，变成 5 次多项式。

### 修复
- 将 baseline 拟合从 `polyfit(..., 5)` 改为 `polyfit(..., 4)`。
- 保持其他 baseline 结构不变：10 段、10 percentile lower envelope、最多 1000 个点、`+0.65 dB` 偏移。

### 待验证
- 该改动会影响 `sbase -> xbase -> xsnr2`，预期主要影响低 SNR 边界消息的接受/拒绝和排序。

### 验证
- `cargo fmt` ✅
- `cargo check --tests` ✅
- `git diff --check` ✅
- `cargo test --release test_stream_decode_short_audio -- --nocapture` ✅
  - `21` unique messages
- `cargo test --release test_stream_decode_long_audio -- --nocapture` ✅
  - `422/449`
  - 每段均小于 `15s`
  - timing residual median `+0.785s`

## Milestone 4: stream 控制流小对齐

### 修复
- `ndepth=1` 时，WSJT-X `ft8_decode.f90` 在 `nzhsym<50` 直接返回，不运行 41/47 early decode。Rust stream 现在在 depth 1 下跳过 41 阶段，最终只跑 50 阶段。
- 外部 `ft8_a7d` 现在受 `ft8_ap` 和 contest 6/7 约束：
  - `lft8apon=false` 时不运行。
  - `ncontest==6` 或 `ncontest==7` 时不运行，匹配 WSJT-X 外层 A7 条件。

### 影响
- 默认长测配置为 `depth=3, ft8_ap=true, ncontest=0`，因此预期不改变当前分数。
- 该修复主要是避免其他配置路径偏离 WSJT-X。

### 验证
- `cargo fmt` ✅
- `cargo check --tests` ✅
- `git diff --check` ✅
- `cargo test --release test_stream_decode_short_audio -- --nocapture` ✅
  - `21` unique messages
- `cargo test --release test_stream_decode_long_audio -- --nocapture` ✅
  - `422/449`
  - 每段均小于 `15s`

## Engineering: CLI 与模块边界整理

### 目的
- 灵敏度追分暂时暂停，先把工程入口和模块边界整理清楚。
- 保持解码核心不和 CLI/UI/文件读取耦合，方便后续继续逐函数对齐 WSJT-X 时减少看错文件、变量和控制流的概率。

### 改动
- 新增 `input::audio` 模块：
  - 负责 WAV 读取、整数/浮点样本转换、多通道折叠为 mono、线性重采样。
- 新增 `stream::time` 模块：
  - 支持 `YYMMDD_HHMMSS`、`YYYYMMDD_HHMMSS`、`HHMMSS` 解析。
- 新增 `stream::slot` 和 `input::file`：
  - `stream::slot` 负责文件样本按 12 kHz / 15 秒 slot 喂给解码 session。
  - 保留同一个 `StreamDecodeSession` 实例跨 slot 运行，继续共享 hashcallbook 和 AP memory。
  - `input::file` 负责 WSJT-X 风格文件名时间戳推断、WAV 文件入口、读取和重采样。
  - EOF 时保留最后一个非空尾 slot，与当前测试 harness 的行为一致。
- 重写 CLI：
  - `ft8rs --fft-engine fftw file <wav> [--start-time ...]`
  - 文件名可推断时间戳时不需要显式 `--start-time`。
  - stdout 只输出解码信息：`timestamp snr dt freq msg`。
  - `soundcard` 子命令先保留为未实现，后续单独接入声卡采集。
- 清理 CLI 输出路径，stdout 只保留解码信息。

### 验证
- `cargo fmt` ✅
- `cargo check --tests` ✅
- `target/release/ft8rs --fft-engine fftw file tests/ft8/210703_133430.wav` ✅
  - 只输出 timestamped decode lines。

### 基线验证
- `cargo test --release test_stream_decode_short_audio -- --nocapture` ✅
  - `21` unique messages
  - 约 `4.4s`
- `cargo test --release test_stream_decode_long_audio -- --nocapture` ✅
  - `422/449`
  - 每段均小于 `15s`
  - timing residual median 仍为 `+0.785s`

## Engineering: CLI 流式输出

### 问题
- 文件 CLI 之前通过 `decode_wav_file` 先收集完整文件的所有解码结果，最后统一输出。
- 这不符合实时/流式使用方式：应该解码完一个 15 秒 slot 就立刻输出这一段。

### 修复
- 新增 `decode_wav_file_streaming`，文件读取和重采样后按 slot 调用回调。
- CLI 改为使用 streaming 回调：
  - 每解完一段立即打印该段解码结果。
  - 段与段之间打印 `====`。
  - 每段输出后 flush stdout。
- README 说明 CLI 是逐段输出，段间用 `====` 分隔。

### 验证
- `cargo fmt` ✅
- `cargo check --tests` ✅
- `cargo run --release -- --fft-engine fftw file tests/ft8/210703_133430.wav` ✅
  - 短文件正常输出 `21` 条。
- `target/release/ft8rs --fft-engine fftw file tests/ft8/230208_140300.wav --low 200 --high 210 --depth 1` ✅
  - 长文件快速 smoke test 输出 19 个 slot 的段间分隔符。

## Engineering: 输入层和 WSJT-X 命名整理

### 目的
- 解码器、流式 slot 适配层、文件入口、声卡入口分层更清楚。
- decoder-facing 参数和关键内部结构向 WSJT-X 命名靠拢，减少继续对照 Fortran 时的错读。

### 改动
- 新增 `stream::slot`：
  - 只负责 12 kHz 样本按 15 秒 slot 驱动 `StreamDecodeSession`。
  - 保留同一个 `StreamDecodeSession` 跨 slot 运行，保持 hashcallbook 和 AP memory。
- 新增 `input` 层：
  - `input::file` 负责 WAV 读取、重采样，并调用 `stream::slot`。
  - `input::soundcard` 保留声卡入口 stub。
- 删除旧 `stream::file`：
  - 避免文件 I/O 和流式 slot 适配混在同一层。
- `StreamDecodeConfig` 改为兼容别名，具体类型为 `WsjtxDecodeConfig`。
- 配置字段向 WSJT-X 对齐：
  - `freq_low/freq_high` -> `nfa/nfb`
  - `depth` -> `ndepth`
  - `max_candidates` -> `ncand`
  - `nqso_progress` -> `nQSOProgress`
  - `ft8_ap/ap_cq_only` -> `lft8apon/lapcqonly`
  - `sync_min` -> `syncmin`
- `ft8::decode::DecodeOptions` 同步改成 decoder-facing WSJT-X 风格字段。
- AP memory 内部结构从通用 `SlotDecodeEntry` 改为 `A7SaveEntry`，字段使用 `msg0/dt0/f0`。

### 验证
- `cargo fmt` ✅
- `cargo check --tests` ✅
