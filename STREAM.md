# STREAM - FT8 Streaming Decode Alignment Report

## 1. Scope

`ft8rs` 的目标是做一个独立、纯粹的 FT8 流式解码模块，并尽量对齐
WSJT-X 的 FT8 接收能力。本文只记录仍然有工程价值的技术结论、当前状
态、测试约束和后续排查重点。

对齐优先级固定为：

1. 源码层面的架构差异。
2. miss 分析暴露出的架构差异。
3. 源码层面的参数差异。
4. miss 分析暴露出的参数差异。

原则：先对齐 WSJT-X，再做不改变解码语义的性能优化。不能为了速度或
局部灵敏度偏离 WSJT-X 的控制流、参数和数据边界。

主要参考源码：

- `wsjtx/lib/jt9a.f90`
- `wsjtx/lib/decoder.f90`
- `wsjtx/lib/ft8_decode.f90`
- `wsjtx/lib/ft8/ft8b.f90`
- `wsjtx/lib/ft8/sync8.f90`
- `wsjtx/lib/ft8/ft8_a7.f90`
- `wsjtx/lib/ft8/ft8_downsample.f90`
- `wsjtx/lib/ft8/get_spectrum_baseline.f90`
- `wsjtx/lib/ft8/subtractft8.f90`
- `wsjtx/lib/77bit/packjt77.f90`

JTDX 源码可以作为参考，但 JTDX 比 WSJT-X 更激进。任何来自 JTDX 的发现
都不能直接落地为默认行为，必须先确认是否符合 WSJT-X 对齐目标。

## 2. Current Baseline

当前受保护基线：

| Fixture | Requirement | Current |
|---|---:|---:|
| `210703_133430.wav` | at least `19/20`, slot under `15s` | `21` unique messages |
| `230208_140300.wav` | current floor `424/449`, each slot under `15s`, slot offset `+0.785s` | `424/449` |

当前观察到的性能：

- short fixture: about `3.2-3.4s` in release mode.
- long fixture: about `63-67s` total in release mode.
- observed slowest long slot: about `4-5s`, below the `15s` streaming limit.

测试规则：

- 解码测试一律使用 `--release`。
- 长测必须保留每段 `15s` 超时约束。
- 长测必须保留灵敏度 early-abort，当前严重失败阈值为 `424-10`。
- 不允许降低基线或放宽性能门槛来通过测试。

常用命令：

```bash
cargo test --release test_stream_decode_short_audio -- --nocapture
cargo test --release test_stream_decode_long_audio -- --nocapture
cargo test --release --features fftw test_stream_decode_short_audio -- --nocapture
cargo test --release --features fftw test_stream_decode_long_audio -- --nocapture
```

## 3. Module Boundaries

当前结构方向：

- `src/ft8`: 解码器核心。拥有 FT8/JT77 协议逻辑、pack/unpack、LDPC、
  AP、hash callbook、CRC、subtraction、WSJT-X 对齐常量和内部工具。
- `src/stream`: 流式解码适配层。负责 slot 切分、时间戳、EOF tail slot、
  `nzhsym=41/47/50` 阶段推进，以及保持一个跨 slot 的
  `StreamDecodeSession`。
- `src/input/file`: 文件输入入口。负责 WAV 读取、单声道折叠、重采样、起
  始时间推断或 `--start-time` 解析。
- `src/input/soundcard`: 声卡输入入口。无 `--device` 时列出输入设备，有
  `--device <index-or-name>` 时监听指定输入设备。
- `src/main.rs`: CLI 参数解析和输出。不能持有解码细节。
- `src/util`: 只保留真正跨模块使用的基础设施，目前主要是 FFT 后端分发。

解码器应保持相对独立。上层可以使用显式暴露的 session/config/result
接口，但不应依赖 FT8 内部实现细节。

CLI 目标：

```bash
ft8rs file tests/ft8/230208_140300.wav
ft8rs file some.wav --start-time 230208_140300
ft8rs monitor
ft8rs monitor --device "VB-Cable A" --slots 2
ft8rs monitor --device "VB-Cable A" --udp --udp-host 127.0.0.1 --udp-port 2238
```

CLI 输出要求：

- 解码结果按 slot 流式输出，先解出的强信号先打印。
- 每个 slot 结束后打印 compact separator，并包含该段 decode count。
- `monitor --udp` 对每条 decode 发送 WSJT-X UDP Decode packet 兼容格式。
  默认 destination 是 `127.0.0.1:2238`，可通过 `--udp-host` 和
  `--udp-port` 修改。未传 `--udp` 时不发送 UDP。
- CLI 普通输出不依赖 `FT8RS_TRACE_TIMERS=1`。

## 4. Audio and Slot Model

WSJT-X FT8 解码边界：

| Item | WSJT-X value | Notes |
|---|---:|---|
| Internal sample rate | `12000 Hz` | 声卡输入先转换到解码采样率。 |
| Decoder buffer | `integer*2 iwave(15*12000)` | FT8 核心处理 15s 窗口。 |
| FT8 frame window | `180000 samples` | `15s * 12000`。 |
| Symbol samples | `NSPS=1920` | 6.25 baud, 0.160s/symbol。 |
| Sync step | `NSTEP=480` | `NSPS/4`, 0.040s lag grid。 |
| Downsample | `NDOWN=60` | 12000 Hz -> 200 Hz。 |
| Sync FFT | `NFFT1=3840` | 3.125 Hz/bin, exactly 2 bins/tone。 |
| Long FFT | `192000 -> 3200` | 16s zero-padded FFT then 200 Hz baseband。 |
| Default band | `200-3000 Hz` | spectrum baseline clamps to 100-4910 Hz。 |

`ft8rs` 文件和声卡入口可以接收不同输入格式，但进入解码器前必须形成
稳定的 12 kHz `f32` stream。FT8 核心仍然按 180000-sample slot 工作。

长文件不是简单丢弃尾部。当前 harness 会在 EOF 时 flush 最后一个非空
tail slot；这对 `230208_140300.wav` 很关键，因为最后的 `230208_140730`
slot 约有 `14.47s` 音频。

## 5. FFT Policy

当前决策：**只保留 3840 FFT size，后端在编译期选择。**

- 默认构建：`RustFFT @ 3840`。
- 对齐测试构建：`FFTW @ 3840`，通过 `--features fftw` 启用。
- 运行时 FFT 后端切换已经移除。
- `rustfft@4096` 已移除。

原因：

- WSJT-X `sync8` 使用 `NFFT1=3840`。
- `12000/3840 = 3.125 Hz/bin`。
- FT8 tone spacing 是 `6.25 Hz`，正好是 2 个 bin。
- 本地 A/B 测试中，`RustFFT@3840` 和 `FFTW@3840` 在旧 no-offset 保护
  fixture 上都保持 `21` 和 `422/449`。切换 `+0.785s` 对齐窗口并修正
  WSJT-X `gen_ft8wave` 包络和 `subtractft8` refined-DT `sqf()` 后，
  RustFFT 与 FFTW 当前都保持 `424/449`。

发布策略：

- GitHub release artifact 使用默认 `RustFFT@3840`，避免 FFTW runtime
  依赖，Windows/Linux 下载后可以直接运行。
- CI 仍跑 `--features fftw` 的 release stream tests，用来保护
  `FFTW@3840` 的 WSJT-X 对齐路径。
- 做 WSJT-X 数值级比较时，优先使用 `--features fftw`。

FT8 解码核心的 FFT 调用命名和缩放策略按 WSJT-X `four2a` 对齐：

- `four2a_r2c(re, im)` 对应 `call four2a(x,n,1,-1,0)`。
- `four2a_c2c(re, im, -1)` 对应 complex forward
  `call four2a(c,n,1,-1,1)`。
- `four2a_c2c(re, im, 1)` 对应 complex inverse
  `call four2a(c,n,1,1,1)`。
- `four2a_c2c` 两个方向都不做 normalization；调用点像 Fortran 一样显式
  乘各自的 `fac`。

这样做的目的不是改变数学结果，而是让 rounding path、函数名、调用方向和
WSJT-X 源码一一对应，减少后续排查时的“等价但不同形”噪音。

## 6. WSJT-X Streaming Control Flow

### 6.1 `nzhsym=41/47/50`

WSJT-X disk-file FT8 decode 会跑渐进式 partial passes：

| `nzhsym` | Input boundary | Behavior |
|---:|---:|---|
| `41` | `41*3456 = 141696` samples | early decode, rest zero-padded |
| `47` | `47*3456 = 162432` samples | subtract selected early decodes, save cleaned early buffer |
| `50` | full 15s slot | combine cleaned early part with original tail, then full decode and AP |

重要点：

- `ndepth=1` 时，WSJT-X 对 `nzhsym<50` 直接返回，所以 depth 1 不跑 early
  partial decode。
- `nzhsym<50` 会禁用 `ft8b` 内部 AP passes。
- stream session 必须跨 partial stages 保留同一个 slot 状态，不能把
  `41/47/50` 当作互不相关的三个普通 decode。

### 6.2 Outer `ft8_decode`

WSJT-X regular outer loop：

```text
npass = 2 when depth == 1, otherwise 3
pass 1: imetric = 1
pass 2: imetric = 2
pass 3: imetric = 2
syncmin = 2.1 when ndepth <= 2, otherwise 1.3
```

Candidate decode order must preserve WSJT-X behavior:

- `sync8` finds candidates on current residual.
- `ft8b` attempts candidates in WSJT-X candidate order.
- Valid codewords are subtracted before outer duplicate filtering, matching
  WSJT-X effective subtract order.
- After each pass, later sync and `sbase` must be computed from the current
  residual, not stale pass-1 state.

### 6.3 `ft8b`

Key WSJT-X behavior currently represented in `ft8rs`:

- coarse downsample, time refine, frequency refine, second downsample, final
  time refine.
- hard sync gate follows WSJT-X:
  - `syncmin=6`, or `7` when `imetric=2`, or `8` when `ndepth<=2`
  - bailout when `nsync <= syncmin`
- regular LLR streams: `llra`, `llrb`, `llrc`, `llrd`, `llre`.
- `imetric=2` squares the temporary `s2` metric before bit extraction.
- `decode174_91` distinguishes channel-LLR OSD from BP-posterior OSD and keeps
  WSJT-X-style acceptance rules.
- AP pass scheduling follows `nappasses` and `naptypes` keyed by
  `nQSOProgress`, with `lapcqonly`, `ncontest`, `lft8apon`, and `nzhsym`
  gates.

Known remaining gaps:

- Deeper `ndeep>=3` LDPC/OSD branches are not fully ported.
- AP masks need more bit-level regression fixtures against WSJT-X, especially
  contest and Hound examples.
- Some long-file misses may still come from subtle windowing, padding, or AP
  memory differences rather than simple sensitivity parameters.

## 7. `sync8` Alignment

Important WSJT-X details:

- `NFFT1=3840`, `df=3.125 Hz`, `JZ=62`。
- `jstrt=0.5/tstep` is assigned to an integer in Fortran, so it truncates to
  `12` rather than rounding to `13`。
- `m`, `m36`, and `m72` are Fortran 1-based time-bin variables. Rust must
  convert to 0-based only at the array access boundary.
- Missing this conversion shifts sync by one `NSTEP` (`480` samples, `0.04s`)。
- `mlag=13` for `red/jpeak`; `mlag2=JZ` for `red2/jpeak2`。
- `red` and `red2` are normalized by the 40th percentile over frequency bins。
- Near-duplicate pruning is candidate-order based: if `df<4 Hz` and
  `dt<0.04s`, keep the stronger sync candidate。
- Candidate order is `nfqso +/- 10 Hz` first, then remaining candidates by
  sync strength。

`sbase` must come from `get_spectrum_baseline(dd,nfa,nfb)`, not from symbol
spectra used for sync. `sbase` indexing follows WSJT-X 1-based convention:
FFT bin 0/DC is omitted and vector index 0 is unused.

## 8. Pack/Unpack and Hash Semantics

Important WSJT-X details:

- Project scope is FT8 receive/decode alignment. JT77 contains WSPR-style
  payload forms, but WSPR itself is not an `ft8rs` target. WSPR-specific
  pack/unpack code is intentionally excluded.
- `pack77`/`unpack77` must cover active WSJT-X `i3/n3` message families.
  Receive decode must not discard a valid LDPC codeword just because its
  77-bit family is uncommon. The current Rust receive side covers:
  - `i3=0,n3=0`: free text.
  - `i3=0,n3=1`: DXpedition special messages such as
    `CALL RR73; CALL <HASH> +00`.
  - `i3=0,n3=3/4`: ARRL Field Day exchange.
  - `i3=0,n3=5`: telemetry.
  - `i3=1/2`: standard messages and `/R`/`/P` forms.
  - `i3=3`: ARRL RTTY contest exchange.
  - `i3=4`: one nonstandard/hash call plus one standard/nonstandard call.
  - `i3=5`: EU VHF contest exchange with hashed calls.
- `i3=0,n3=6` is treated as out of scope for this project and rejected before
  message unpacking.
- `split77` / `pack77_1` use `chkcall`, which is stricter than AP-style
  `stdcall` checks。
- `chkcall` tests call-area position 2 and then 3 with assignment order that
  matters; Rust must not collapse this into an `else if`。
- `is_stdcall()` is intentionally looser and remains separate from the
  `pack77` internal callsign parser。
- Receive `unpack77` needs the same `mycall/hiscall` hash context that
  WSJT-X initializes in `ft8b` before calling `unpack77(c77,1,...)`。 This
  affects display and duplicate/AP memory for 10/12/22-bit hashed calls.
- Type 1 `R GRID` checks the third word (`parts[2]` in Rust)。
- Two-word Type 1 messages reject a second word containing `/`。
- `unpack77` rejects invalid CQ report/grid combinations; `RR73` is a special
  trap because it also looks like a 4-character grid。
- Resolved hash display forms such as `<RK4FF>` and `RK4FF` should be treated
  as equivalent for test diff matching, while unresolved `<...>` remains
  distinct。

`HashCallBook` is shared across stream slots through the decode session. Upper
layers must not create independent callbooks per slot.

## 9. AP and Cross-slot Memory

WSJT-X `ft8_a7` behavior:

- `ft8_a7_save` stores decoded fragments by even/odd `jseq`。
- On new UTC or `nzhsym=41`, current entries are moved from `k=1` to `k=0`
  for that parity。
- AP at `nzhsym=50` uses previous entries for the same parity。
- Entries containing `/` or `<` are skipped。
- If a current decode has the same second call and near frequency as a previous
  AP candidate, the previous entry is suppressed (`f0=-98`)。
- `ft8_a7d` brute-forces message variants and accepts only with WSJT-X-style
  `dmin`, `dmin2/dmin`, CQ/grid, and SNR guards。

Current `ft8rs` status:

- Same-parity previous/current AP memory is represented in the stream session。
- Current regular decodes suppress near previous AP candidates。
- AP results preserve refined `freq` and `dt`。
- `ft8_a7d` sync refinement uses `ctwk * Costas` for frequency tweak and plain
  Costas sync for second time refinement。
- `ft8_a7d` SNR keeps WSJT-X's `pbest/xbase/3e6` divisor because AP `s8` is
  kept at `abs(csymb)` scale。
- Decoded tokens are filtered before saving into `HashCallBook`, avoiding grid
  and report tokens such as `FN20` or `RR73`。

Remaining AP risk:

- Exact `ndec(jseq,k)` storage is still simplified compared with Fortran arrays。
- CQ fragment extraction should continue to be checked against `split77` word
  classification when debugging weak CQ misses。
- AP bit masks need direct fixtures against WSJT-X-generated patterns。

## 10. `subtractft8` Indexing

WSJT-X keeps `nstart` and `j` as 1-based sample indices:

```fortran
nstart=dt*12000+1 + idt
do i=1,nframe
   j=nstart-1+i
   if(j.ge.1.and.j.le.NMAX) camp(i)=dd(j)*conjg(cref(i))
enddo
```

Rust must map `i=0` to the same Fortran sample `j=nstart`:

```rust
let j = nstart_1based + rust_i as isize;
let sample = dd0[(j - 1) as usize];
```

Using `j=nstart-1+rust_i` shifts coherent subtraction by one sample. This
affects envelope estimation, refined DT, and residual writeback.

`subtractft8` 的 LPF 也按源码结构对齐：

- `NFFT=NMAX=180000` circular FFT filter。
- `cw(1:NFILT+1)=window/sumw` 后执行 `cshift(cw,NFILT/2+1)`。
- forward `four2a` 后显式 `cw=cw*fac`，其中 `fac=1/NFFT`。
- 对 `cfilt` 做 forward FFT、乘 `cw`、inverse FFT，再应用首尾
  `endcorrection`。

`gen_ft8wave` complex envelope shaping now matches WSJT-X:

- first ramp: `(1-cos(angle))/2`。
- last ramp: `(1+cos(angle))/2`。

Earlier no-offset experiments made this look unsafe in isolation, but under
the current `+0.785s` aligned fixture window it improves the protected RustFFT
long score and is retained as a source-aligned correction.

`subtractft8` refined-DT now mirrors the WSJT-X internal `sqf()` structure:

- trial offsets `-90/+90/0` each rebuild a local `dd` from `dd0` and run a full
  subtract without mutating the caller buffer。
- while `ldt=true`, `sqf()` computes the post-subtraction FFT energy only in
  the FT8 signal band `f0-1.5*baud .. f0+8.5*baud`。
- `sqf` band energy and `peakup(sqa,sq0,sqb,dx)` use single-precision `real`
  semantics, matching the Fortran declarations。
- `peakup` selects `i2=nint(90*dx)`; a final `sqf(i2)` with `ldt=false`
  performs the only writeback。

With the current aligned fixture window this raises the protected RustFFT long
score to `424/449` while keeping every slot below `15s`。

## 11. Recording Start Offset Diagnostic

The long-file harness keeps a diagnostic based on matched messages:

```text
baseline_drift - decoded_dt
```

For `230208_140300.wav`, the no-offset estimate was stable:

- median around `+0.785s`
- p10/p90 around `+0.745..+0.825s`

This suggests the WAV sample 0 may be closer to `230208_140300.785` than
exactly `230208_140300.000`。The long-test harness now uses `+0.785s` as the
default slot-start offset so future miss analysis compares against a
time-aligned window:

- `0.785s` centers timing residual near zero and currently scores `424/449`。
- no-offset previously scored `422/449` but carried a median residual around
  `+0.785s`。
- `0.500s` produced the best temporary sweep score observed so far (`426/449`)。
- Larger offsets increase late large-drift misses。

Interpretation: this is more likely a WSJT-X file windowing, padding,
continuous-buffer, or AP-memory alignment issue than a simple timestamp
correction. Treat `+0.785s` as the current aligned fixture window, not as a
general decoder parameter.

Keep the diagnostic and saved offset comparison files for future investigation.

## 12. Performance Notes

Performance work must not alter sensitivity-related parameters, candidate
search space, AP pass semantics, or residual subtract order.

No-algorithm-change optimizations already applied:

- cache LDPC generator matrix with `OnceLock`。
- reuse OSD work buffers in the inner pattern loop。
- encode into existing codeword buffers instead of allocating temporary `Vec`s。
- use unstable reliability sorting where sort stability has no semantic role。
- decode worker owns one `StreamDecodeSession` while input thread continues
  consuming soundcard audio。

Optional timer tracing:

```bash
FT8RS_TRACE_TIMERS=1 cargo test --release test_stream_decode_short_audio -- --nocapture
```

Timer tracing is silent by default and must not appear in normal CLI output.

## 13. GitHub Release Workflow

Current release workflow:

- Triggers on push to `main` and manual dispatch。
- Runs FFTW acceptance tests on Linux with `--features fftw`。
- Builds release artifacts with default `RustFFT@3840`。
- Builds Linux and Windows artifacts。
- macOS artifact is temporarily disabled because hosted runner queue time is too
  long。
- Release artifacts do not require FFTW runtime libraries。

## 14. Near-term Priorities

1. Continue source-level architecture comparison in `ft8_decode`、`ft8b`、
   `ft8_a7`、`sync8`、`ft8_downsample`。
2. Use miss-only diff to locate architecture gaps before changing parameters。
3. Keep the recording-start offset diagnostic while comparing file windowing、
   padding、continuous-buffer behavior and AP memory。
4. Add focused fixtures where they do not slow acceptance work:
   AP mask bits, baseline numerical parity, candidate ordering, EOF tail slot,
   and representative hash display forms。
5. Only after control-flow parity is accounted for, use source and miss analysis
   to audit remaining parameter differences。

Current source-level finding:

- WSJT-X carries early decodes into the final `nzhsym=50` decode via
  `ndecodes=ndec_early` and the saved `allmessages` table. ft8rs now mirrors
  that with `DecodeOptions.initial_messages`: early messages seed duplicate and
  pass-control state, but are not returned again by the full-stage decoder.
- This alignment kept the previous no-offset long-file score unchanged at
  `422/449`; after switching the default long-test window to `+0.785s` and
  aligning `gen_ft8wave` envelope shaping plus `subtractft8` refined-DT
  `sqf()`, the protected RustFFT baseline is `424/449`.
- Current numeric-homology cleanup:
  - FT8 core call sites now use `four2a_r2c` / `four2a_c2c` instead of generic
    normalized FFT wrappers.
  - `ft8_downsample` and AP downsample use the WSJT-X inverse FFT path plus
    `fac=1/sqrt(NFFT1*NFFT2)` directly.
  - `sync8` uses source-shaped `nfos=NFFT1/NSPS`。
  - `ft8b` `bmete` and `ft8_a7d` time-refine use first-max behavior matching
    Fortran `maxloc`。
  - LDPC `platanh` now uses WSJT-X's piecewise approximation and `±7.0`
    saturation instead of exact `atanh`; this keeps BP/OSD iteration numerics
    on the same path as `platanh.f90`。
  - OSD reliability ordering now uses a local port of WSJT-X `indexx` before
    reversing to MRB order, instead of Rust's generic unstable sort。
  - `sync8` percentile normalization, candidate ordering, and final sync sort
    now use the same `indexx` ordering shape as `sync8.f90`; FT8 baseline
    percentile selection uses the same helper as well。
  - FT8 spectrum-baseline `nuttal_window` constants and signs now match
    WSJT-X `lib/nuttal_window.f90` exactly:
    `0.3635819, -0.4891775, 0.1365995, -0.0106411`。
  - `subtractft8` reference waveform envelope now matches
    `gen_ft8wave.f90` with `(1-cos(angle))/2` and `(1+cos(angle))/2` ramps。
  - `subtractft8` refined-DT now uses WSJT-X's `sqf()` shape: local `dd`
    rebuild per trial, signal-band FFT energy under `ldt=true`, and final
    one-time writeback using `i2=nint(90*dx)`。The energy accumulator and
    `peakup` arithmetic are narrowed to f32 to match Fortran `real`/`real*4`
    behavior.
  - Previous no-offset release validation remained `21` short and `422/449`
    long; current `+0.785s` RustFFT validation is `424/449` long.
- `ft8_a8d` remains a known architecture gap. It is only active when AP is on,
  contest is not Fox/Hound, `nzhsym=50`, `hiscall` and `hisgrid` are populated,
  and the a7 path has not already decoded the target. It does not explain the
  default long-file baseline without QSO context, but should be kept on the
  WSJT-X parity list.
- Focused diagnosis of the strongest remaining miss,
  `230208_140430 F4JAR UX7UU -19`, shows the candidate reaches `ft8b` with
  strong hard sync (`nsync=18`). At the selected refined time, LDPC/OSD can
  sometimes recover the correct CRC/message, but reports about `40` hard
  errors, so both WSJT-X and ft8rs reject it via the `nharderrors<=36` gate.
  A temporary time sweep of the same downsampled signal found nearby `ibest`
  values, about `+7` 200 Hz samples later, where the same message decodes with
  `33..34` hard errors. The miss is therefore localized to `ft8b` time
  refinement / soft-symbol extraction parity, not candidate admission, message
  formatting, LDPC reachability, or FFT backend.
- A focused pass-level diagnostic on `seg=6/f≈1413` gave the same shape with
  RustFFT and FFTW: selected `ibest=-46`, `nsync=18`, and the target message
  appears in regular LDPC passes with hard errors around `40..43`. The current
  rejection is therefore a strict threshold outcome at the selected soft-symbol
  alignment, not an unpack/type/AP/FFT-backend failure.
- The inner LDPC pass control now mirrors WSJT-X more closely: if a CRC-good
  codeword fails all-zero, message-type, unpack, or contest-specific checks,
  ft8rs continues later passes just like WSJT-X `cycle` inside `ft8b.f90`.
  The message-type guard was also corrected to allow `i3=0,n3=6`, matching
  `if(i3.gt.5 .or. (i3.eq.0.and.n3.gt.6)) cycle`.
- `ft8_downsample` now mirrors the WSJT-X scaling order: the `NFFT2` inverse
  FFT is left unnormalized and the caller applies
  `fac=1/sqrt(NFFT1*NFFT2)` exactly once. This replaces the previous
  mathematically equivalent normalized-inverse path
  `(1/NFFT2)*sqrt(NFFT2/NFFT1)`, eliminating a possible rounding-path
  difference for future weak-signal audits. AP downsample uses the same helper.
  The `cshift(c1,i0-ib)` step also uses signed modular indexing to match the
  Fortran shift semantics without unsigned underflow at low-frequency edges.
- A temporary single-precision `fftwf` probe that mirrors WSJT-X
  `ft8_downsample.f90` and `sync8d.f90` selected the same peak for this target
  (`ibest=-46` without file offset, equivalently `ibest=111` with a `+0.785s`
  window offset). The global file offset diagnostic improves displayed timing
  residuals but does not recover this decode. This points away from FFT
  precision or simple file-start offset as single causes, and toward outer
  multi-pass residual/AP state or neighboring-signal interactions.
- A no-subtract diagnostic for this target left the same `ibest`/hard-error
  pattern unchanged, so the single miss is not explained by previous accepted
  signals being subtracted from the residual. A separate duplicate/subtract
  ordering experiment reduced the long baseline to `411/449` and remains
  rejected; WSJT-X's effective regular path subtracts inside `ft8b` before the
  outer duplicate filter sees the message.
- `nagain` full-stage behavior now follows `ft8_decode.f90`: when `nzhsym=50`
  and `nagain=true`, the decoder uses the original full slot rather than the
  early-cleaned residual, while the inner decoder searches only `nfqso±20Hz`.
  Default streaming tests keep `nagain=false`, so this is an architecture
  alignment change rather than a sensitivity-tuning change.
- `subtractft8` LPF now follows the WSJT-X structure more closely: 180000-point
  circular FFT filtering with a cshifted cos² window and endpoint correction,
  instead of the previous 262144-point halo/linear-convolution approximation.
  This kept both RustFFT and FFTW no-offset long baselines at `422/449`, while
  changing a few edge diff rows, confirming residual subtraction is active in
  the remaining boundary behavior.
- `subtractft8` refined-DT now follows the WSJT-X `sqf()` energy and side-effect
  structure. This was not safe as an isolated old-window edit, but became
  baseline-positive after the `+0.785s` window and `gen_ft8wave` envelope were
  aligned.
- `subtractft8` `nstart` now follows Fortran implicit-integer truncation:
  `nstart=dt*12000+1+idt`. This was source-aligned under the previous
  no-offset `422/449` baseline.
- `gen_ft8wave` envelope shaping now uses the WSJT-X formulas
  `(1-cos(angle))/2` and `(1+cos(angle))/2`; with the `+0.785s` fixture window
  this is both source-aligned and baseline-safe.
- `ft8b` AP pass selection no longer has an extra `ndepth>=2` gate. WSJT-X
  controls these passes with `lapon/ncontest/nzhsym` and then forces regular
  passes only when `nzhsym<50`.

## 15. Documentation Policy

Active project documents:

- `README.md`: user-facing overview and CLI/build examples。
- `STREAM.md`: technical alignment report and current status。
- `TRY.md`: compact attempt log。

Other Markdown reports should either be removed or folded into `STREAM.md` /
`TRY.md`。
