# WSJT-X Alignment Notes

本文是 `ft8rs` 当前唯一的 WSJT-X 对齐技术记录。它合并了原 `STREAM.md`
和 `TRY.md` 中仍有价值的内容：架构边界、参数、测试基线、关键踩坑、已经
确认的对齐点、拒绝过的尝试，以及仍未完全贴合 WSJT-X 的地方。

`README.md` 只保留用户入口、编译、CLI 和测试命令。

## Scope

`ft8rs` 的目标是做一个独立、纯粹的 FT8 流式解码模块，尽量对齐 WSJT-X
的 FT8 接收能力。JTDX 可以作为参考，但默认行为必须先对齐 WSJT-X。

对齐优先级固定为：

1. 源码层面的架构差异。
2. miss 分析暴露出的架构差异。
3. 源码层面的参数差异。
4. miss 分析暴露出的参数差异。

原则：先对齐 WSJT-X，再做不改变解码语义的性能优化；不能为了速度或局部
灵敏度偏离 WSJT-X 的控制流、参数和数据边界。

主要参考源码：

- `wsjtx/lib/ft8_decode.f90`
- `wsjtx/lib/ft8/ft8b.f90`
- `wsjtx/lib/ft8/sync8.f90`
- `wsjtx/lib/ft8/ft8_a7.f90`
- `wsjtx/lib/ft8/ft8_downsample.f90`
- `wsjtx/lib/ft8/subtractft8.f90`
- `wsjtx/lib/ft8/decode174_91.f90`
- `wsjtx/lib/ft8/osd174_91.f90`
- `wsjtx/lib/77bit/packjt77.f90`

## Current Baseline

| Fixture | Requirement | Current |
|---|---:|---:|
| `210703_133430.wav` | at least `19/20`, slot under `15s` | `21` unique messages |
| `230208_140300.wav` | WSJT-X target floor `425`, slot under `15s`, no fixture offset | `425/425` target rows |

测试规则：

- 解码测试一律使用 `--release`。
- 长测必须保留每段 `15s` 超时约束。
- 长测保留灵敏度 early-abort，当前严重失败阈值为 `425-10`。
- 不允许通过降低 `ncand/ndepth`、关闭 AP、放宽门限或扩大非 WSJT-X 搜索来追分。
- `230208_140300.csv` 的 `Extra` 列用于标记来源：
  - 空值：多重验证基线，属于 WSJT-X target。
  - `W`：WSJT-X 额外解码，属于 WSJT-X target。
  - `J`：JTDX 额外解码，保留参考但不进入 WSJT-X miss/diff。
  - `E`：其他或问题解码，保留参考但不进入 WSJT-X miss/diff。

常用命令：

```bash
cargo test --release test_stream_decode_short_audio -- --nocapture
cargo test --release test_stream_decode_long_audio -- --nocapture
cargo test --release --features fftw test_stream_decode_short_audio -- --nocapture
cargo test --release --features fftw test_stream_decode_long_audio -- --nocapture
```

## Module Boundaries

当前结构：

- `src/ft8`: FT8 解码核心。拥有 FT8/JT77 协议逻辑、pack/unpack、LDPC、AP、
  hash callbook、CRC、subtraction、WSJT-X 对齐常量和内部工具。
- `src/stream`: 流式适配层。负责 slot 切分、时间戳、EOF tail slot、
  `nzhsym=41/47/50` 阶段推进，以及跨 slot 的 `StreamDecodeSession`。
- `src/input/file`: WAV 文件入口，负责读取、单声道折叠、重采样和起始时间。
- `src/input/soundcard`: 声卡入口。无 `--device` 时列出输入设备，有
  `--device <index-or-name>` 时监听指定输入设备。
- `src/output`: 输出层。目前有 CLI 输出和 UDP Decode packet 输出。
- `src/main.rs`: CLI 参数解析和 input/output 组合，不承载解码细节。
- `src/util`: 只保留真正跨模块使用的基础设施，目前主要是 FFT 后端分发。

解码器应保持相对独立。上层可以使用显式暴露的 session/config/result 接口，
但不应依赖 FT8 内部实现细节。

### Deliberate Rust Naming

Rust 代码不完全使用 Fortran 大小写，但语义尽量贴近：

- `nfa/nfb/ndepth/nfqso/nftx/ncontest/nzhsym/ncand` 保持 WSJT-X 风格。
- `nqso_progress` 对应 WSJT-X `nQSOProgress`。
- `enabled/cq_only` 分别对应 `lft8apon/lapcqonly`。
- `ft8_a7d`、`ft8_downsample`、`sync8`、`subtract_ft8` 等核心函数名保留
  WSJT-X 源码名，方便逐段核对。

### File-layout Alignment

FT8 regular decode 的主要阶段按 `wsjtx/lib/ft8/*.f90` 镜像到
`src/ft8/lib/ft8/*.rs`。Rust 对外仍通过 `crate::ft8::decode` 暴露稳定 API，
但物理文件名尽量和 WSJT-X 保持同名，方便熟悉 WSJT-X 的开发者直接定位：

- `src/ft8/lib/ft8/ft8_decode.rs`: 对外 decode facade、outer `ft8_decode`
  控制流、public decode API 和 sample preparation。对应
  `wsjtx/lib/ft8_decode.f90`，但放在 `lib/ft8` 下以保持 FT8 解码器聚合。
- `src/ft8/lib/ft8/ft8_params.rs`: WSJT-X `ft8_params.f90` style constants。
- `src/ft8/lib/ft8/workspace.rs`: Rust work buffers plus candidate/result/AP
  option structs；这些是 Fortran 局部数组/记录的 Rust 聚合，没有单独 f90。
- `src/ft8/lib/ft8/baseline.rs`: WSJT-X `baseline.f90` /
  `get_spectrum_baseline.f90` shaped spectrum baseline helpers。
- `src/ft8/lib/ft8/sync_templates.rs`: Costas/taper/frequency-tweak template
  builders；对应 `sync8.f90`、`sync8d.f90`、`ft8_downsample.f90` 中的模板数据。
- `src/ft8/lib/ft8/sync8d.rs`: WSJT-X `sync8d.f90` Costas sync power helper。
- `src/ft8/lib/ft8/symbols.rs`: shared 32-sample symbol FFT extraction helper；
  对应 `ft8b.f90` 和 `ft8_a7.f90` 中相同的 `csymb` 提取形状。
- `src/ft8/lib/ft8/sync8.rs`: `sync8` candidate search、
  baseline-normalized sync metrics、candidate pruning/order。
- `src/ft8/lib/ft8/ft8b.rs`: `ft8b` candidate decode、soft-symbol extraction、
  bit metrics、LDPC/AP pass scheduling、SNR/post gates。
- `src/ft8/lib/ft8/ft8_downsample.rs`: 192k -> 3200 -> 200 Hz downsample path。
- `src/ft8/lib/ft8/ft8_a7.rs`: WSJT-X `ft8_a7.f90:ft8_a7d` AP brute-force
  decoder。
- `src/ft8/lib/ft8/decode174_91.rs`: WSJT-X `decode174_91.f90` /
  `bpdecode174_91.f90` / `osd174_91.f90` LDPC decoder。
- `src/ft8/lib/ft8/ldpc_174_91_c_parity.rs`: WSJT-X
  `ldpc_174_91_c_parity.f90` parity-check table。
- `src/ft8/lib/ft8/subtractft8.rs`: WSJT-X `subtractft8.f90` residual
  subtraction。
- `src/ft8/lib/77bit/packjt77.rs`: WSJT-X `77bit/packjt77.f90` pack side。
- `src/ft8/lib/77bit/unpack77.rs`: WSJT-X `77bit/packjt77.f90` receive
  unpack side split into its own Rust file to keep pack/unpack readable。
- `src/ft8/lib/77bit/hashcall.rs`: receive hash-call table around
  `packjt77.f90:ihashcall` and WSJT-X runtime callbook behavior。
- `src/ft8/lib/77bit/protocol.rs`: Rust grouping for shared 77-bit alphabets,
  LDPC generator hex strings and protocol constants pulled from `packjt77.f90`,
  `ft8_params.f90` and `ldpc_174_91_c_generator.f90`。
- `src/ft8/lib/indexx.rs`: WSJT-X `indexx.f90` helper used by sync and OSD。

这些镜像文件通过 `src/ft8/mod.rs` 的 `#[path = "lib/ft8/..."]` 挂载到
既有 Rust module 名称，所以上层 stream/input/output 不需要知道内部物理
路径。核心阶段不使用文本 `include!` 合并作用域。

## Audio and Slot Model

WSJT-X FT8 解码边界：

| Item | WSJT-X value | Notes |
|---|---:|---|
| Internal sample rate | `12000 Hz` | 声卡输入先转换到解码采样率 |
| Decoder buffer | `15*12000` samples | FT8 核心处理 15s 窗口 |
| Symbol samples | `NSPS=1920` | 6.25 baud, 0.160s/symbol |
| Sync step | `NSTEP=480` | `NSPS/4`, 0.040s lag grid |
| Downsample | `NDOWN=60` | 12000 Hz -> 200 Hz |
| Sync FFT | `NFFT1=3840` | 3.125 Hz/bin, exactly 2 bins/tone |
| Long FFT | `192000 -> 3200` | 16s zero-padded FFT then 200 Hz baseband |
| Default band | `200-3000 Hz` | spectrum baseline clamps to 100-4910 Hz |

文件和声卡入口可以接收不同输入格式，但进入解码器前必须形成稳定的
12 kHz stream。长文件不能简单丢弃尾部；EOF 时需要 flush 最后一个非空
tail slot。

## FFT Policy

当前只保留 `3840` FFT size，后端在编译期选择：

- 默认构建：`RustFFT @ 3840`。
- 对齐测试构建：`FFTW @ 3840`，通过 `--features fftw` 启用。
- 运行时 FFT 后端切换已移除。
- `rustfft@4096` 已移除。

原因：WSJT-X `sync8` 使用 `NFFT1=3840`，`12000/3840=3.125 Hz/bin`，
FT8 tone spacing `6.25 Hz` 正好是 2 个 bin。

发布策略：

- Release artifact 使用默认 `RustFFT@3840`，避免 FFTW runtime 依赖。
- CI 跑 `--features fftw` 的 release stream tests，保护 WSJT-X 对齐路径。
- 做 WSJT-X 数值级比较时，优先使用 `--features fftw`。

FT8 核心 FFT 调用命名和缩放策略按 WSJT-X `four2a` 对齐：

- `four2a_r2c(re, im)` 对应 `call four2a(x,n,1,-1,0)`。
- `four2a_c2c(re, im, -1)` 对应 complex forward。
- `four2a_c2c(re, im, 1)` 对应 complex inverse。
- 两个方向都不做 normalization；调用点像 Fortran 一样显式乘各自的 `fac`。
- `ft8_downsample` 的 `fac=1/sqrt(float(NFFT1)*NFFT2)` 按 WSJT-X 默认
  `real` 路径保留为 `f32`，再写回 Rust 的 f64 work buffers；regular 和 AP
  downsample 共用同一个缩放语义。

## WSJT-X Streaming Control Flow

### `nzhsym=41/47/50`

WSJT-X disk-file FT8 decode 会跑渐进式 partial passes：

| `nzhsym` | Input boundary | Behavior |
|---:|---:|---|
| `41` | `41*3456 = 141696` samples | early decode, rest zero-padded |
| `47` | `47*3456 = 162432` samples | subtract selected early decodes, save cleaned early buffer |
| `50` | `50*3456 = 172800` samples | combine cleaned early part with original tail, zero-pad rest, then full decode and AP |

重要点：

- `ndepth=1` 时，WSJT-X 对 `nzhsym<50` 直接返回。
- `nzhsym<50` 会禁用 `ft8b` 内部 AP passes。
- stream session 必须跨 partial stages 保留同一个 slot 状态。
- final `nzhsym=50` 也要在 `50*3456` 后补零，不能直接使用完整 15s buffer 的
  尾部 `7200` samples。

### Outer `ft8_decode`

WSJT-X regular outer loop：

- `npass=2` when depth 1, otherwise `3`。
- pass 1 uses `imetric=1`; pass 2/3 use `imetric=2`。
- `syncmin=2.1` when `ndepth<=2`, otherwise `1.3`。
- `sync8` 在当前 residual 上找候选。
- `ft8b` 按 WSJT-X candidate order 尝试候选。
- 每个 pass 后，后续 sync 和 `sbase` 必须来自当前 residual。

### `ft8b`

当前已对齐的关键行为：

- coarse downsample -> time refine -> frequency refine -> second downsample -> final time refine。
- hard sync gate：`syncmin=6`，`imetric=2` 时 `7`，`ndepth<=2` 时 `8`；
  bailout when `nsync <= syncmin`。
- regular LLR streams：`llra/llrb/llrc/llrd/llre`。
- `imetric=2` squares temporary `s2` before bit extraction。
- `try_decode_passes` 对 CRC-good 但 post-gate 不合法的 codeword 继续后续 pass，
  对齐 WSJT-X `cycle` 语义。
- AP pass scheduling follows current `wsjtx/lib/ft8/ft8b.f90`：
  `npasses=5+2*nappasses(nQSOProgress)`，`lapcqonly=>7`，`nzhsym<50=>5`。
- AP magnitude follows current WSJT-X FT8, not FT4:
  `apmag=maxval(abs(llrz))*1.1` after selecting the current pass metric.

Known fixture gaps：

- LDPC/OSD needs more independent WSJT-X-generated golden vectors beyond the
  current source-shape and release audio tests。

## `sync8` and Baseline

Important WSJT-X details:

- `NFFT1=3840`, `df=3.125 Hz`, `JZ=62`。
- `jstrt=0.5/tstep` assigned to integer, so it truncates to `12`。
- `m/m36/m72` 是 Fortran 1-based time-bin；Rust 只在数组访问边界转成 0-based。
- 漏掉这个转换会把 sync 整体错开一个 `NSTEP` (`480` samples, `0.04s`)。
- `mlag=13` for `red/jpeak`; `mlag2=JZ` for `red2/jpeak2`。
- `red/red2` normalized by the 40th percentile over frequency bins。
- near-dupe pruning 按 candidate order；`df<4 Hz` and `dt<0.04s` 时保留更强 sync。
- candidate order 是 `nfqso +/- 10 Hz` first, then remaining candidates by sync strength。

`sbase` must come from `get_spectrum_baseline(dd,nfa,nfb)`，不是 symbol spectra。
`sbase` indexing follows WSJT-X 1-based convention: FFT bin 0/DC is omitted and
vector index 0 is unused。

## Pack/Unpack and Hash Semantics

项目范围是 FT8 receive/decode。JT77 中的 WSPR-style payload forms 不作为
`ft8rs` 目标，WSPR-specific pack/unpack code intentionally excluded。

Receive side should cover active WSJT-X FT8 `i3/n3` families:

- `i3=0,n3=0`: free text。
- `i3=0,n3=1`: DXpedition special messages。
- `i3=0,n3=3/4`: ARRL Field Day exchange。
- `i3=0,n3=5`: telemetry。
- `i3=1/2`: standard messages and `/R`/`/P` forms。
- `i3=3`: ARRL RTTY contest exchange。
- `i3=4`: one nonstandard/hash call plus one standard/nonstandard call。
- `i3=5`: EU VHF contest exchange with hashed calls。

Important gates:

- `i3=0,n3=6` is out of scope and rejected before message unpacking。
- `split77` / `pack77_1` use `chkcall`, stricter than AP-style `stdcall`。
- `chkcall` tests call-area position 2 and then 3 with assignment order that matters。
- Type 3 的两个 28-bit callsign slots are validated as exchange callsign tokens。
  `CQ`/`QRZ`/`DE` special tokens are rejected there because WSJT-X `pack77_3`
  reaches Type 3 through `chkcall` and cannot transmit those tokens in the
  callsign fields。
- Receive `unpack77` needs the same `mycall/hiscall` hash context that WSJT-X
  initializes in `ft8b` before `unpack77(c77,1,...)`。
- Diff matching treats resolved hash display forms such as `<RK4FF>` and `RK4FF`
  as equivalent, while unresolved `<...>` remains distinct。

`HashCallBook` is shared across stream slots through the decode session. Upper
layers must not create independent callbooks per slot。

## AP and Cross-slot Memory

WSJT-X `ft8_a7` behavior:

- `ft8_a7_save` stores decoded fragments by even/odd `jseq`。
- On new UTC or `nzhsym=41`, current entries move from `k=1` to `k=0` for that parity。
- AP at `nzhsym=50` uses previous entries for the same parity。
- Entries containing `/` or `<` are skipped。
- Current regular decodes suppress near previous AP candidates with same second call。
- `ft8_a7d` accepts only with WSJT-X-style `dmin`, `dmin2/dmin`, CQ/grid, and SNR guards。

Current `ft8rs` status:

- Same-parity previous/current AP memory is represented in stream session。
- File and monitor paths pass the slot timestamp into the stream session, so
  AP parity uses WSJT-X `jseq = mod(nutc/5,2)` instead of a timestamp-free toggle。
- AP storage now mirrors WSJT-X `ndec(jseq,k)` shape: `a7[jseq][0]` is previous
  same-parity memory, `a7[jseq][1]` is current memory, and `nzhsym=41`/new slot
  moves current to previous for that parity。
- Current regular decodes suppress near previous AP candidates。
- AP results preserve refined `freq` and `dt`。
- `ft8_a7d` sync refinement uses `ctwk * Costas` for frequency tweak and plain
  Costas sync for second time refinement。
- AP symbol extraction uses the shared WSJT-X-shaped `four2a_c2c(...,-1)`
  wrapper for the 32-point symbol FFT。
- AP `s8` is kept at `abs(csymb)` scale, so `ft8_a7d` keeps WSJT-X
  `pbest/xbase/3e6` divisor。

Remaining AP risk:

- AP bit masks now have direct bit-position fixtures for CQ, mycall,
  mycall+dxcall, RRR, 73, RR73 and key contest/call gates。Remaining fixture
  gap is WSJT-X-generated golden bit patterns for every `ncontest/iaptype`
  combination。The current Rust fixture matrix covers accept/reject and mask
  shape/count for `ncontest=0..8` and `iaptype=1..6`。
- AP memory shape is aligned: `a7[jseq][0/1]` stores `msg0` as fixed-width
  uppercase `character*37`-style text plus `dt0/f0`。`call_1/call_2/grid4`
  are derived from `msg0` at AP decode time, matching `ft8_decode.f90`。
  `xbase` is recomputed from the current slot `sbase(max(1,nint(f1/3.125)))`
  before `ft8_a7d`, rather than saved from the previous slot。
  Current tests pin the main `ft8_a7_save` parsing edges: `CQ` with grid,
  `CQ_` skip, report-as-blank grid, and `/`/`<` skip。
- AP and regular decode now share the same `ft8_downsample_from_cx` implementation
  for bin extraction, taper, `cshift`, inverse `four2a`, and `fac` scaling。
- AP and regular decode also share `sync8d/sync8d_twk` Costas sync helpers and
  `extract_symbol_spectrum` for the 32-point symbol FFT。

## Subtraction and Waveform

`subtractft8` indexing follows WSJT-X 1-based sample variables:

```fortran
nstart=dt*12000+1 + idt
do i=1,nframe
   j=nstart-1+i
   if(j.ge.1.and.j.le.NMAX) camp(i)=dd(j)*conjg(cref(i))
enddo
```

Rust maps `i=0` to the same Fortran sample:

```rust
let j = nstart_1based + rust_i as isize;
let sample = dd0[(j - 1) as usize];
```

Other retained alignment:

- LPF uses 180000-point circular FFT filter, cshifted cos² window and endpoint correction。
- refined-DT mirrors WSJT-X `sqf()`：local `dd` rebuild per trial, signal-band FFT
  energy under `ldt=true`, final one-time writeback using `i2=nint(90*dx)`。
- `gen_ft8wave` envelope matches WSJT-X: first ramp `(1-cos(angle))/2`, last ramp
  `(1+cos(angle))/2`。

## Important Pitfalls Fixed

These were the most expensive or easy-to-miss alignment bugs:

- `sync8d` time indexing must not wrap. WSJT-X uses signed indices and contributes
  zero outside the buffer; modulo wrap pulls energy from the end of `cd0`。
- `sync8` Fortran 1-based time-bin access caused a full `0.04s` offset until fixed。
- `subtractft8` `nstart/j` conversion must preserve Fortran's 1-based loop exactly。
- Old `230208_140300.wav` had a stable `+0.785s` recording-start offset and 48 kHz
  format; the active fixture is now normalized to 12 kHz with inserted leading silence。
- AP parity must use WSJT-X `jseq=mod(nutc/5,2)` from the slot timestamp; simple
  toggling can put AP memory in the wrong parity。
- `pack_jt77::is_stdcall()` and stream AP `chkcall` both needed careful 1-based
  call-area conversion。
- Type 3 false positive `CQ 001 IZ7MMG 549 2025` was fixed by validating the two
  RTTY callsign slots against WSJT-X `pack77_3/chkcall` structure, not by disabling
  Type 3 or contest messages。
- Resolved hash display forms such as `<RK4FF>` and `RK4FF` need robust diff
  matching; unresolved `<...>` must remain distinct。
- Earlier suspicion about FT8 AP `npasses` and `apmag` was rechecked against current
  `wsjtx/lib/ft8/ft8b.f90`: `5+2*nappasses`, `lapcqonly=>7`, and `*1.1` are current
  WSJT-X FT8 behavior. The `*1.01` note belongs to other/older commented paths,
  not the active current FT8 `ft8b` path.

## Performance Notes

Performance work must not alter sensitivity-related parameters, candidate search
space, AP pass semantics, or residual subtract order。

No-algorithm-change optimizations already applied:

- cache LDPC generator matrix with `OnceLock`。
- reuse OSD work buffers and avoid repeated codeword allocation。
- delete unused pass-loop FFT。
- reuse candidate workspace per pass。
- avoid repeated hard-sync / SNR tone work。
- input thread can continue consuming soundcard audio while decode worker runs。

Rejected or low-priority:

- candidate parallelism: risks changing duplicate/subtract/residual order。
- FFTW wisdom / FFTW threads: FFT is not the current dominant cost。
- broad `sync8` f32 rewrites: previously reduced long score。
- candidate-level `cd0` cache: removed from the active regular path. WSJT-X shares
  the long 192k FFT per pass, but each `ft8b` candidate still down-samples at its
  own `f1`; keeping a candidate cache made later audits easier to misread and did
  not improve sensitivity。
- duplicate-gated regular subtract: rejected for now; current effective path
  preserves the behavior that recovered the WSJT-X target baseline。
- forcing local `ibest` offsets for misses: useful diagnosis, not a committed heuristic。

Timer tracing is explicit and silent by default:

```bash
FT8RS_TRACE_TIMERS=1 cargo test --release test_stream_decode_short_audio -- --nocapture
```

## Scope Decisions and Deferred Work

- FT8 receive/decode is the project scope. WSPR-style Type 0.6 payloads are not
  implemented as FT8 targets。
- SuperFox / special modern modes were investigated but are not part of the
  current WSJT-X FT8 baseline。
- `ft8bvar` / JTDX more aggressive paths are references only; do not import those
  behaviors unless the goal changes away from WSJT-X parity。
- AP golden fixtures are intentionally not tracked as remaining work for now.
  Existing AP tests still cover active bit positions, gates, message edge cases
  and source-shaped control flow, but the project will not block completion on a
  full independent AP byte-for-byte fixture matrix。
- The current WSJT-X-shaped file split mirrors `wsjtx/lib/**/*.f90` under
  `src/ft8/lib/**/*.rs`; `ft8_params.rs` mirrors WSJT-X parameter naming,
  `workspace.rs` holds Rust-only buffers/types, and no core stage relies on
  textual `include!` glue。
- `tests/wsjtx_source_audit_test.rs` can compare selected Rust source shapes
  against a local `../wsjtx/lib/ft8` checkout. These tests currently cover
  `ft8_params.f90`, `ft8_downsample.f90`, `sync8d.f90` and the deep
  `osd174_91.f90` path, and skip cleanly when the WSJT-X source tree is not
  present.
- Direct fixtures are still needed for broader AP generated patterns, baseline
  numerical parity, EOF tail slot, and hash display forms。
- Deeper `ndeep>=3` LDPC/OSD source shape is now represented, including the
  WSJT-X `npre2` pair-pattern path. Current FT8 `ft8b` still calls `norder=2`,
  so this is mostly completeness and future-audit coverage rather than an active
  sensitivity-path change。

## Remaining Alignment Backlog

These are the known regrets to fix one by one. Keep each item guarded by release
baseline tests, and prefer WSJT-X-generated golden fixtures over hand-derived
expectations wherever possible.

1. **LDPC/OSD golden vectors**: add independent WSJT-X-generated decode vectors
   for BP-only, BP+OSD channel-LLR, saved-BP OSD and deep `ndeep>=3` OSD cases.
   The Rust source shape now includes the `npre2` path, but fixture proof is still
   thinner than desired.
2. **Fixture breadth**: current audio fixtures are strong but limited. Add more
   WSJT-X-verified recordings for contest messages, hash calls, AP progression,
   band-edge signals, collisions and high drift.

## Release Workflow

Current release workflow:

- Triggers on push to `main` and manual dispatch。
- Runs FFTW acceptance tests on Linux with `--features fftw`。
- Builds release artifacts with default `RustFFT@3840`。
- Builds Linux and Windows artifacts。
- macOS artifact is currently disabled because hosted runner queue time is too long。
- Release artifacts do not require FFTW runtime libraries。

## Milestone Summary

### Milestone 1: 361 -> 381

- Converted stream decoder from one-shot full decode to WSJT-X-style
  `nzhsym=41/47/50` progressive flow。
- Fixed `sync8d` out-of-range behavior: signed index + zero contribution instead
  of modulo wrap。

### Milestone 2: 381 -> 401

- Aligned pass FFT lifetime: refresh long FFT per outer pass, not after every
  subtract inside the pass。
- Fixed outer `syncmin`: depth 1/2 uses `2.1`, depth 3 uses `1.3`。
- Fixed `sync8` Fortran 1-based time-bin access。
- AP `sync8d` frequency tweak uses `ctwk * Costas`; second time refine uses plain Costas sync。
- Fixed `stdcall()` 0-based conversion, restoring weak CQ AP templates for calls
  such as `F1PPH`、`R6KEE`、`IW1PUR`。

### Milestone 3: 401 -> 422

- Fixed several 1-based/0-based and message gate issues:
  - `subtractft8` sample index。
  - `pack77_1` `R GRID` third-word check。
  - `split77/chkcall` standard callsign checks。
  - `unpack77` CQ invalid guards。
  - stream AP memory `chkcall` digit-position semantics。
- Long-file harness decodes EOF tail slot instead of dropping it。
- Diff CSV output fixed to stable columns and more robust message matching。
- Recording-start diagnostic found stable `+0.785s` timing residual。

### Milestone 4: 422 -> 425 target rows

- The old `48 kHz` fixture's `+0.785s` start offset was folded into a new
  normalized `12 kHz` fixture, so tests no longer need an offset parameter。
- `gen_ft8wave` envelope and `subtractft8` refined-DT `sqf()` were aligned to WSJT-X。
- Many numeric-homology cleanups were made in FFT/downsample, `ft8b`, `ft8_a7d`,
  LDPC/OSD, `sync8` ordering, and `nuttall_window`。
- Stream AP parity now derives `jseq` from slot `nutc` using WSJT-X
  `mod(nutc/5,2)`。
- AP symbol extraction uses the shared `four2a_c2c(...,-1)` wrapper instead of a
  local 32-point FFT。
- CSV baseline semantics were corrected: blank and `W` rows are the WSJT-X target
  set, while `J`/`E` rows stay in the fixture but are ignored for WSJT-X miss/diff。
- Current release long test reaches `425/425` WSJT-X target rows; the earlier
  `UT7UJ IV3KEI JN65` weak miss is recovered in both target slots。
- AP memory was tightened to the WSJT-X table model: `A7SaveEntry` stores
  `msg0/dt0/f0`, derives `call_1/call_2/grid4` from fixed-width `msg0`, and
  recomputes AP `xbase` from the current slot `sbase` before `ft8_a7d`。
- AP `ft8_a7d` and regular `ft8b` now share the exact same downsample helper,
  eliminating the duplicate taper/downsample implementation in AP。
- `sync8d.f90` behavior and the 32-point symbol FFT extraction are shared by AP
  and regular decode through `sync8d.rs` and `symbols.rs`。
- `osd174_91` now includes the WSJT-X `ndeep>=3` `npre2` pair-pattern path
  (`boxit91/fetchit91` equivalent) even though current FT8 `ft8b` still uses
  `norder=2`。

## Recent Validation

- `cargo test --release test_stream_decode_short_audio -- --nocapture`
  - `21` unique messages。
- `FT8RS_WRITE_DIFF=1 cargo test --release test_stream_decode_long_audio -- --nocapture`
  - total `434/458`。
  - WSJT-X target `425/425`。
  - timing residual median near `+0.000s`。
  - every slot under `15s`。
  - diff file contains only the header。
- `cargo test --release wsjtx_ -- --nocapture`
  - source-audit tests passed for `ft8_params`, `ft8_downsample`, `sync8d` and
    deep `osd174_91` shape when the local WSJT-X source tree was present。
- `cargo test --release decode174 -- --nocapture`
  - OSD deep-path smoke tests passed。
