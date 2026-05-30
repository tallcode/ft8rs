# ft8rs

`ft8rs` 是一个以 WSJT-X 为对齐目标的 FT8 流式解码器。当前重点不是做一个独立风格的 FT8 解码实现，而是尽量在架构、参数、时间窗口、padding、FFT、AP、跨时隙记忆和性能约束上贴近 WSJT-X。

项目目标：

- 提供一个独立的纯解码模块，不和 UI 耦合。
- 支持从 WAV 文件按 FT8 slot 流式解码，输出带时间戳的解码行。
- 支持指定声卡，按系统时间切分音频流并实时输出解码结果。
- 默认使用 `rustfft @ 3840`，无需外部 FFTW 运行库。
- 可在编译时启用 `FFTW @ 3840`，作为 WSJT-X 对齐验证路径。

当前状态：

- 文件流式 CLI 已可用。
- 声卡输入 CLI 可列出输入设备，并可按系统时间对齐 FT8 slot 后实时采集解码。
- 主要验收基线仍以 release 模式测试为准。
- README 只记录如何编译、运行和验证；详细 WSJT-X 对齐记录见 `WSJTX.md`。

## 环境要求

需要 Rust stable toolchain。

默认构建使用 `rustfft @ 3840`，只需要 Rust stable toolchain。

如果要运行 WSJT-X 对齐验证路径，可启用 `fftw` feature；此时系统需要能链接
`libfftw3`。在 macOS 上通常可以用：

```bash
brew install fftw
```

启用 FFTW 编译：

```bash
cargo build --release --features fftw
```

编译后不能在运行时切换 FFT 引擎：默认产物是 `rustfft @ 3840`，带
`--features fftw` 的产物是 `FFTW @ 3840`。

FFTW 构建支持 WSJT-X `jt9` 风格的 FFT 线程参数：

```bash
cargo build --release --features fftw
target/release/ft8rs file tests/ft8/210703_133430.wav -m 3 -w 1
```

`--fft-threads` 也可写作 `-m`。默认值为 1。默认 RustFFT 构建没有 FFTW
那种单个 plan 内部多线程接口，因此 `--fft-threads > 1` 会明确报错。
`--patience` 也可写作 `-w`，取值 0..=4，默认值为 1；默认 RustFFT 构建
只接受默认值 1。

## 编译 CLI

推荐始终编译 release 版本：

```bash
cargo build --release
```

编译完成后 CLI 位于：

```bash
target/release/ft8rs
```

构建 FFTW 对齐版本：

```bash
cargo build --release --features fftw
```

查看帮助：

```bash
target/release/ft8rs --help
target/release/ft8rs file --help
target/release/ft8rs monitor --help
```

## 运行 CLI

### 从文件名推断起始时间

如果 WAV 文件名包含 WSJT-X 风格时间戳，例如 `210703_133430.wav`，可以直接运行：

```bash
target/release/ft8rs file tests/ft8/210703_133430.wav
```

输出格式：

```text
HHMMSS SNR DT FREQ MESSAGE
```

示例输出：

```text
133430  -5 +0.3  2571 W1FC F5BZB -08
133430  -7 -0.1  2157 WM3PEN EA6VQ -09
133430 -25 -0.8  1197 CQ F5RXL IN94
```

CLI 的 stdout 只输出解码信息和段结束分隔符。文件会按 15 秒 slot 流式解码，并在解码阶段内逐条输出：`nzhsym=41` 早期结果会先打印，随后补充完整 slot 和 AP 阶段新结果。每段消息后面紧跟分隔符，并标出本段解码数量：

```text
------ slot done: 14 decodes ------
```

### 显式指定起始时间

如果文件名没有时间戳，使用 `--start-time`：

```bash
target/release/ft8rs file recording.wav --start-time 230208_140300
```

也可以只指定当天时间：

```bash
target/release/ft8rs file recording.wav --start-time 140300
```

### 解码参数

CLI 参数按用途分组。常用短别名如下：

- Decode context: `-c/--my-call`、`-G/--my-grid`、`-x/--his-call`、
  `-g/--his-grid`、`-Q/--qso-progress`
- Frequency: `-L/--low`、`-H/--high`、`-f/--rx-frequency`、
  `-T/--tx-frequency`、`-A/--ap-width`
- Decode: `-d/--depth`、`-C/--max-candidates`、`-P/--no-ap`、
  `-O/--cq-only`
- FFTW: `-m/--fft-threads`、`-w/--patience`
- Input/output: `-s/--start-time`、`-i/--device`、`-S/--slots`、
  `-u/--udp`、`-o/--udp-host`、`-p/--udp-port`

指定频率范围、深度和候选数量：

```bash
target/release/ft8rs file tests/ft8/230208_140300.wav \
  -L 200 \
  -H 3000 \
  -d 3 \
  -C 1000 \
  -m 1 \
  -w 1
```

### 指定 AP/Hash 上下文

`file` 和 `monitor` 都支持 WSJT-X `jt9` 风格的上下文参数：

```bash
target/release/ft8rs file tests/ft8/230208_140300.wav \
  -c K1ABC \
  -G FN20 \
  -x W9XYZ \
  -g EN60 \
  -Q 0
```

其中 `--my-call` 和 `--his-call` 会参与 WSJT-X 风格 AP 解码和 hash-call
unpack；`--my-grid` 和 `--his-grid` 作为正式 decode config 保留，用于和
WSJT-X CLI 参数形态对齐。`--qso-progress` / `-Q` 取值为 `0..=5`，用于
WSJT-X 风格 AP pass 选择。

FFTW 构建下可以用 `-m` / `--fft-threads` 指定大 FFT plan 线程数：

```bash
target/release/ft8rs file tests/ft8/230208_140300.wav -m 3 -w 1
```

禁用 AP 解码：

```bash
target/release/ft8rs file tests/ft8/230208_140300.wav -P
```

### 实时监听入口

不带 `--device` 时列出系统输入设备：

```bash
target/release/ft8rs monitor
```

输出会标出输入设备索引、host、设备名、默认设备标记和默认输入格式。

带 `--device` 时进入声卡采集解码入口。`--device` 可以使用上面列表里的 `Index`，也可以使用完整设备名：

```bash
target/release/ft8rs monitor -i 0
target/release/ft8rs monitor -i "MacBook Pro Microphone"
```

测试时可以限制监听段数：

```bash
target/release/ft8rs monitor -i "VB-Cable A" -S 2
```

默认只输出 CLI。加上 `--udp` 后，`monitor` 会把每条解码结果按
WSJT-X UDP Decode packet 兼容格式发给 UDP report destination。默认目的
地址是 `127.0.0.1:2238`，可以修改：

```bash
target/release/ft8rs monitor -i "VB-Cable A" -u
target/release/ft8rs monitor -i "VB-Cable A" -u -o 127.0.0.1 -p 2238
```

声卡输入按系统 UTC 时间对齐到下一个 15 秒 slot 后开始采集，每段采集完成后进入同一套渐进解码输出路径：早期结果先打印，完整 slot 和 AP 阶段继续补充新结果。每段消息后面使用同样的分隔符，并标出本段解码数量。

## 测试

所有解码验收测试都必须使用 release 模式。debug 模式耗时没有参考意义。

短音频测试：

```bash
cargo test --release test_stream_decode_short_audio -- --nocapture
```

FFTW 对齐路径测试：

```bash
cargo test --release --features fftw test_stream_decode_short_audio -- --nocapture
```

当前要求：

- 文件：`tests/ft8/210703_133430.wav`
- 至少匹配 `19/20`
- 单段耗时小于 `15s`
- 当前基线通常为 `21` unique messages

长音频测试：

```bash
cargo test --release test_stream_decode_long_audio -- --nocapture
```

FFTW 对齐路径长测：

```bash
cargo test --release --features fftw test_stream_decode_long_audio -- --nocapture
```

当前要求：

- 文件：`tests/ft8/230208_140300.wav`
- 当前 WSJT-X target 保护线：`425`
- 测试 WAV 已标准化为 `12 kHz / mono / 16-bit`，sample 0 对齐文件名时间戳，
  harness 不再使用额外 slot offset
- CSV `Extra` 列中，空值和 `W` 属于 WSJT-X target baseline；`J`/`E` 只保留
  作参考，不参与当前 WSJT-X miss/diff
- 每个 15 秒片段耗时必须小于 `15s`
- 测试 harness 带有灵敏度保护，严重低于基线会提前失败

一次通过时的摘要类似：

```text
[STREAM LONG DECODE SUMMARY]
  Total matched: 434/458 (94.8%)
  WSJT-X baseline matched: 425/425
  Timing residual: baseline_drift-decoded_dt mean=-0.016s median=+0.000s p10=-0.043s p90=+0.040s n=434
```

排查 miss/extra 时可以写出 diff 文件：

```bash
FT8RS_WRITE_DIFF=1 cargo test --release test_stream_decode_long_audio -- --nocapture
```

生成的 diff 用于观察 `-` miss 和 `+` extra，匹配逻辑会对部分展示差异做归一化，例如 `<CALL>` 和 `CALL`。

## 工程结构

当前按四层划分：

- `src/decode`: 独立解码核心，当前聚焦 FT8，继续按 WSJT-X `lib` 下的 `ft8_decode`、`ft8b`、`ft8_a7` 对齐。
  FT8/JT77 协议内部模块也归属这里，例如 pack/unpack、LDPC、hashcall、subtract 和协议常量。
- `src/stream`: 流式 slot 适配层，负责 12 kHz / 15 秒 slot 驱动、跨 slot `HashCallBook`、同奇偶 AP memory。
- `src/input`: 输入入口层，当前包含文件入口和声卡入口。
- `src/main.rs`: CLI 参数解析和逐 slot 解码行输出。

辅助模块：

- `src/input/audio.rs`: WAV 读取、多声道折叠、重采样。
- `src/stream/time.rs`: slot 时间戳解析和格式化。
- `src/input/file.rs`: 文件名时间戳推断、WAV 文件入口和流式文件解码。

decoder 参数命名尽量贴近 WSJT-X，例如 `nfa`、`nfb`、`ndepth`、`nQSOProgress`、`lft8apon`、`lapcqonly`、`nzhsym`。后续继续对齐源码时，应优先保持这些命名和 WSJT-X 控制流的一一对应关系。

`src/util` 只保留真正跨层的 FFT 基础设施。只有 FT8 解码器使用的协议工具不放在 `util`，而是放回 `src/decode`。

## License

GPL-3.0
