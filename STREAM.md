# STREAM - FT8 Streaming Decode Alignment

本文只保留当前仍有工程价值的状态、架构边界、WSJT-X 对齐规则和下一步
排查重点。历史尝试流水放在 `TRY.md`，用户入口和 CLI 用法放在 `README.md`。

## 1. Scope

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
- `wsjtx/lib/77bit/packjt77.f90`

## 2. Current Baseline

| Fixture | Requirement | Current |
|---|---:|---:|
| `210703_133430.wav` | at least `19/20`, slot under `15s` | `21` unique messages |
| `230208_140300.wav` | WSJT-X target floor `425`, each slot under `15s`, no fixture offset | `425/425` target rows |

测试规则：

- 解码测试一律使用 `--release`。
- 长测必须保留每段 `15s` 超时约束。
- 长测保留灵敏度 early-abort，当前严重失败阈值为 `425-10`。
- 不允许通过降低 `ncand/ndepth`、关闭 AP、放宽门限或扩大非 WSJT-X 搜索来追分。
- `230208_140300.csv` 的 `Extra` 列用于标记来源：空值表示多重验证基线，
  `W` 表示 WSJT-X 额外解码，这两类都属于当前 WSJT-X 对齐目标；`J` 表示
  JTDX 额外解码，`E` 表示其他/问题解码，这两类暂不进入 miss/diff 关注范围。

常用命令：

```bash
cargo test --release test_stream_decode_short_audio -- --nocapture
cargo test --release test_stream_decode_long_audio -- --nocapture
cargo test --release --features fftw test_stream_decode_short_audio -- --nocapture
cargo test --release --features fftw test_stream_decode_long_audio -- --nocapture
```

## 3. Module Boundaries

当前结构方向：

- `src/ft8`: 解码器核心。拥有 FT8/JT77 协议逻辑、pack/unpack、LDPC、AP、
  hash callbook、CRC、subtraction、WSJT-X 对齐常量和内部工具。
- `src/stream`: 流式适配层。负责 slot 切分、时间戳、EOF tail slot、
  `nzhsym=41/47/50` 阶段推进，以及跨 slot 的 `StreamDecodeSession`。
- `src/input/file`: WAV 文件入口，负责读取、单声道折叠、重采样和起始时间。
- `src/input/soundcard`: 声卡入口。无 `--device` 时列出输入设备，有
  `--device <index-or-name>` 时监听指定输入设备。
- `src/output`: 输出层。目前有 CLI 输出和 UDP Decode packet 输出。
- `src/main.rs`: CLI 参数解析和 input/output 组合，不承载解码细节。
- `src/util`: 只保留真正跨模块使用的基础设施，目前主要是 FFT 后端分发。

解码器应保持相对独立。上层可以使用显式暴露的 session/config/result
接口，但不应依赖 FT8 内部实现细节。

## 4. Audio and Slot Model

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

## 5. FFT Policy

当前只保留 `3840` FFT size，后端在编译期选择：

- 默认构建：`RustFFT @ 3840`。
- 对齐测试构建：`FFTW @ 3840`，通过 `--features fftw` 启用。
- 运行时 FFT 后端切换已移除。
- `rustfft@4096` 已移除。

原因：WSJT-X `sync8` 使用 `NFFT1=3840`，`12000/3840=3.125 Hz/bin`，
FT8 tone spacing `6.25 Hz` 正好是 2 个 bin。

发布策略：

- Release artifact 使用默认 `RustFFT@3840`，避免 FFTW runtime 依赖。
- CI 仍跑 `--features fftw` 的 release stream tests，保护 WSJT-X 对齐路径。
- 做 WSJT-X 数值级比较时，优先使用 `--features fftw`。

FT8 核心 FFT 调用命名和缩放策略按 WSJT-X `four2a` 对齐：

- `four2a_r2c(re, im)` 对应 `call four2a(x,n,1,-1,0)`。
- `four2a_c2c(re, im, -1)` 对应 complex forward。
- `four2a_c2c(re, im, 1)` 对应 complex inverse。
- 两个方向都不做 normalization；调用点像 Fortran 一样显式乘各自的 `fac`。

## 6. WSJT-X Streaming Control Flow

### `nzhsym=41/47/50`

WSJT-X disk-file FT8 decode 会跑渐进式 partial passes：

| `nzhsym` | Input boundary | Behavior |
|---:|---:|---|
| `41` | `41*3456 = 141696` samples | early decode, rest zero-padded |
| `47` | `47*3456 = 162432` samples | subtract selected early decodes, save cleaned early buffer |
| `50` | `50*3456 = 172800` samples | combine cleaned early part with original tail, zero-pad the rest, then full decode and AP |

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
- valid codeword 的 subtract 时序按 WSJT-X effective regular path 保留。
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
- AP pass scheduling follows `nappasses` and `naptypes` keyed by `nQSOProgress`，
  with `lapcqonly/ncontest/lft8apon/nzhsym` gates。

Known gaps：

- deeper `ndeep>=3` LDPC/OSD branches are not fully ported。
- AP masks 还需要更多 bit-level fixtures。
- 部分剩余 miss 可能来自 windowing、padding、AP memory 或 soft-symbol 边界。

## 7. `sync8` and Baseline

重要 WSJT-X 细节：

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

## 8. Pack/Unpack and Hash Semantics

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

## 9. AP and Cross-slot Memory

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
  AP parity uses WSJT-X `jseq = mod(nutc/5,2)` instead of a timestamp-free
  toggle。
- Current regular decodes suppress near previous AP candidates。
- AP results preserve refined `freq` and `dt`。
- `ft8_a7d` sync refinement uses `ctwk * Costas` for frequency tweak and plain
  Costas sync for second time refinement。
- AP symbol extraction uses the shared WSJT-X-shaped `four2a_c2c(...,-1)`
  wrapper for the 32-point symbol FFT。
- AP `s8` is kept at `abs(csymb)` scale, so `ft8_a7d` keeps WSJT-X
  `pbest/xbase/3e6` divisor。

Remaining AP risk:

- Exact `ndec(jseq,k)` storage is still simplified compared with Fortran arrays。
- AP bit masks need direct fixtures against WSJT-X-generated patterns。

## 10. Subtraction and Waveform

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

## 11. Recording Start Offset Diagnostic

The old `230208_140300.wav` fixture was `48 kHz / 32-bit` and its sample 0 was
about `+0.785s` later than the timestamp in the filename. That file and the
old offset comparison CSVs are kept under `tests/old/`。

The active `tests/ft8/230208_140300.wav` fixture is normalized:

- `12 kHz / mono / 16-bit PCM`。
- `285.000s`, exactly 19 FT8 slots。
- `0.785s` of leading silence was inserted before the old audio, so sample 0
  aligns with `230208_140300`。
- The long-test harness now slices slots directly with `slot_start_offset=0`。

The long-file harness still prints a residual diagnostic based on matched
messages:

```text
baseline_drift - decoded_dt
```

For the active normalized fixture, median residual should stay near `+0.000s`。
This keeps the fixture usable for comparison with other FT8 decoders that cannot
handle the old sample-rate and start-offset quirks。

## 12. Performance Notes

Performance work must not alter sensitivity-related parameters, candidate
search space, AP pass semantics, or residual subtract order。

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

Timer tracing is explicit and silent by default:

```bash
FT8RS_TRACE_TIMERS=1 cargo test --release test_stream_decode_short_audio -- --nocapture
```

## 13. Release Workflow

Current release workflow:

- Triggers on push to `main` and manual dispatch。
- Runs FFTW acceptance tests on Linux with `--features fftw`。
- Builds release artifacts with default `RustFFT@3840`。
- Builds Linux and Windows artifacts。
- macOS artifact is currently disabled because hosted runner queue time is too long。
- Release artifacts do not require FFTW runtime libraries。

## 14. Near-term Priorities

1. Continue source-level architecture comparison in `ft8_decode`、`ft8b`、
   `ft8_a7`、`sync8`、`ft8_downsample`。
2. Use miss-only diff to locate architecture gaps before changing parameters。
3. Keep the recording-start offset diagnostic while comparing file windowing、
   padding、continuous-buffer behavior and AP memory。
4. Add focused fixtures where useful and cheap: AP mask bits, baseline numerical
   parity, candidate ordering, EOF tail slot, and hash display forms。
5. Only after control-flow parity is accounted for, use source and miss analysis
   to audit remaining parameter differences。

## 15. Active Documents

- `README.md`: user-facing overview and CLI/build examples。
- `STREAM.md`: technical alignment report and current status。
- `TRY.md`: compact attempt log。

Other Markdown reports should either be removed or folded into `STREAM.md` /
`TRY.md`。
