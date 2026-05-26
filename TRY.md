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
- 长测试分段数改为 `ceil(duration/15s)`，让文件流结束时的非空尾段也进入 `StreamDecoder::decode_slot`。
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
