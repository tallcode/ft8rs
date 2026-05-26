# ft8rs

`ft8rs` 是一个以 WSJT-X 为对齐目标的 FT8 流式解码器。当前重点不是做一个独立风格的 FT8 解码实现，而是尽量在架构、参数、时间窗口、padding、FFT、AP、跨时隙记忆和性能约束上贴近 WSJT-X。

项目目标：

- 提供一个独立的纯解码模块，不和 UI 耦合。
- 支持从 WAV 文件按 FT8 slot 流式解码，输出带时间戳的解码行。
- 后续支持指定声卡，按系统时间切分音频流并实时输出解码结果。
- 默认使用 `FFTW @ 3840` 对齐 WSJT-X，同时保留 `rustfft @ 4096` 作为便携编译和对比路径。

当前状态：

- 文件流式 CLI 已可用。
- 声卡输入 CLI 入口已预留，但采集后端尚未实现。
- 主要验收基线仍以 release 模式测试为准。
- README 只记录如何编译、运行和验证；详细对齐记录见 `STREAM.md`，迭代尝试见 `TRY.md`。

## 环境要求

需要 Rust stable toolchain。

默认 FFT 引擎是 FFTW，因此系统需要能链接 `libfftw3`。在 macOS 上通常可以用：

```bash
brew install fftw
```

如果只是需要更方便地编译和运行，可使用 `rustfft` 引擎：

```bash
cargo build --release --features force-rustfft
```

注意：WSJT-X 对齐和正式验收优先使用默认 `FFTW @ 3840`。

## 编译 CLI

推荐始终编译 release 版本：

```bash
cargo build --release
```

编译完成后 CLI 位于：

```bash
target/release/ft8rs
```

查看帮助：

```bash
target/release/ft8rs --help
target/release/ft8rs file --help
```

## 运行 CLI

### 从文件名推断起始时间

如果 WAV 文件名包含 WSJT-X 风格时间戳，例如 `210703_133430.wav`，可以直接运行：

```bash
target/release/ft8rs --fft-engine fftw file tests/ft8/210703_133430.wav
```

输出格式：

```text
YYMMDD_HHMMSS SNR DT FREQ MESSAGE
```

示例输出：

```text
210703_133430  -5 +0.3  2571 W1FC F5BZB -08
210703_133430  -7 -0.1  2157 WM3PEN EA6VQ -09
210703_133430 -25 -0.8  1197 CQ F5RXL IN94
```

CLI 的 stdout 只输出解码信息。文件会按 15 秒 slot 流式解码，解完一段立即输出一段；段与段之间使用 `====` 分隔。

### 显式指定起始时间

如果文件名没有时间戳，使用 `--start-time`：

```bash
target/release/ft8rs --fft-engine fftw file recording.wav --start-time 230208_140300
```

也可以只指定当天时间：

```bash
target/release/ft8rs --fft-engine fftw file recording.wav --start-time 140300
```

### 指定频率范围和深度

```bash
target/release/ft8rs --fft-engine fftw file tests/ft8/230208_140300.wav \
  --low 200 \
  --high 3000 \
  --depth 3 \
  --max-candidates 1000
```

禁用 AP 解码：

```bash
target/release/ft8rs --fft-engine fftw file tests/ft8/230208_140300.wav --no-ap
```

### 使用 rustfft 引擎

运行时切换到 `rustfft @ 4096`：

```bash
target/release/ft8rs --fft-engine rustfft file tests/ft8/210703_133430.wav
```

`rustfft` 路径主要用于便携编译和对照，不作为 WSJT-X 对齐结论的默认依据。

### 声卡入口

声卡命令入口已预留：

```bash
target/release/ft8rs soundcard --device default
```

当前会返回未实现。声卡采集、系统时间分段和实时输出会在后续单独接入和验证。

## 测试

所有解码验收测试都必须使用 release 模式。debug 模式耗时没有参考意义。

短音频测试：

```bash
cargo test --release test_stream_decode_short_audio -- --nocapture
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

当前要求：

- 文件：`tests/ft8/230208_140300.wav`
- 当前保护线：`422/449`
- 第四里程碑目标：`430/449`
- 每个 15 秒片段耗时必须小于 `15s`
- 测试 harness 带有灵敏度保护，严重低于基线会提前失败

一次通过时的摘要类似：

```text
[STREAM LONG DECODE SUMMARY]
  Total matched: 422/449 (94.0%)
  Timing offset estimate: start_offset=baseline_drift-decoded_dt mean=+0.760s median=+0.785s p10=+0.745s p90=+0.825s n=422
```

排查 miss/extra 时可以写出 diff 文件：

```bash
FT8RS_WRITE_DIFF=1 cargo test --release test_stream_decode_long_audio -- --nocapture
```

生成的 diff 用于观察 `-` miss 和 `+` extra，匹配逻辑会对部分展示差异做归一化，例如 `<CALL>` 和 `CALL`。

## 工程结构

当前按四层划分：

- `src/ft8`: FT8 单 slot 解码核心，继续按 WSJT-X `ft8_decode`、`ft8b`、`ft8_a7` 对齐。
  FT8/JT77 协议内部模块也归属这里，例如 pack/unpack、LDPC、hashcall、subtract 和协议常量。
- `src/stream`: 流式 slot 适配层，负责 12 kHz / 15 秒 slot 驱动、跨 slot `HashCallBook`、同奇偶 AP memory。
- `src/input`: 输入入口层，当前包含文件入口和声卡 stub。
- `src/main.rs`: CLI 参数解析和逐 slot 解码行输出。

辅助模块：

- `src/input/audio.rs`: WAV 读取、多声道折叠、重采样。
- `src/stream/time.rs`: slot 时间戳解析和格式化。
- `src/input/file.rs`: 文件名时间戳推断、WAV 文件入口和流式文件解码。

decoder 参数命名尽量贴近 WSJT-X，例如 `nfa`、`nfb`、`ndepth`、`nQSOProgress`、`lft8apon`、`lapcqonly`、`nzhsym`。后续继续对齐源码时，应优先保持这些命名和 WSJT-X 控制流的一一对应关系。

`src/util` 只保留真正跨层的 FFT 基础设施。只有 FT8 解码器使用的协议工具不放在 `util`，而是放回 `src/ft8`。

## License

GPL-3.0
